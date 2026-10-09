//! Streaming directory-summary sidecar: the incremental feed and the framed
//! sink. Mirrors `emit_records` / `write_sidecar_from_order` record for
//! record so spilled builds publish byte-identical sidecars.

use crate::dir_summary::{
    ancestor_paths, close_stack, codec, compare_paths, directory_summary_path, normalize_path,
    open_entry, open_private_file, temporary_path, DirectorySummaryEntry, HashingWriter, OpenEntry,
    MAGIC, VERSION,
};
use crate::FileRecord;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

/// Incremental form of `emit_records`: the same directory aggregation one
/// record at a time, over records already in summary order.
pub(crate) struct SummaryFeed {
    stack: Vec<OpenEntry>,
    previous: Option<(u32, Box<str>)>,
}

impl SummaryFeed {
    pub(crate) fn new() -> Self {
        Self {
            stack: Vec::new(),
            previous: None,
        }
    }

    pub(crate) fn push(
        &mut self,
        record: &FileRecord,
        emit: &mut impl FnMut(DirectorySummaryEntry) -> io::Result<()>,
    ) -> io::Result<()> {
        let previous = self.previous.replace((record.source, record.path.clone()));
        let normalized_path = normalize_path(record.path.as_ref())?;
        if let Some((source, path)) = previous {
            if source == record.source
                && compare_paths(path.as_ref(), &normalized_path) == std::cmp::Ordering::Equal
            {
                // Native lanes can expose aliases, but an exact source/path
                // pair must contribute once to prevent duplicate totals.
                return Ok(());
            }
        }
        let ancestors = ancestor_paths(&normalized_path);
        let desired = ancestors
            .get(..ancestors.len().saturating_sub(1))
            .unwrap_or_default();
        let common = self
            .stack
            .iter()
            .zip(desired)
            .take_while(|(entry, path)| entry.source == record.source && entry.path == **path)
            .count();
        close_stack(&mut self.stack, common, emit)?;
        for path in &desired[common..] {
            open_entry(&mut self.stack, record.source, path);
        }
        if record.kind == crate::FileKind::Dir {
            for entry in &mut self.stack {
                entry.directory_count = entry.directory_count.saturating_add(1);
            }
            open_entry(&mut self.stack, record.source, &normalized_path);
            return Ok(());
        }
        let contributes_file = matches!(
            record.kind,
            crate::FileKind::File | crate::FileKind::Symlink
        );
        for entry in &mut self.stack {
            if contributes_file {
                entry.logical_bytes = entry.logical_bytes.saturating_add(record.size);
                entry.physical_bytes = entry.physical_bytes.saturating_add(record.disk_bytes());
                entry.file_count = entry.file_count.saturating_add(1);
            }
        }
        if let Some(parent) = self.stack.last_mut() {
            parent.children.push(crate::DirectoryChild {
                path: normalized_path.clone().into_boxed_str(),
                kind: record.kind,
                logical_bytes: record.size,
                physical_bytes: record.disk_bytes(),
                file_count: u64::from(contributes_file),
                directory_count: 0,
            });
        }
        Ok(())
    }

    pub(crate) fn finish(
        mut self,
        emit: &mut impl FnMut(DirectorySummaryEntry) -> io::Result<()>,
    ) -> io::Result<()> {
        self.previous = None;
        close_stack(&mut self.stack, 0, emit)
    }
}

/// Framed sidecar writer: same header, streaming entries, checksum trailer,
/// atomic publish as `write_sidecar_from_order`.
pub(crate) struct SidecarSink {
    encoder: Option<zstd::stream::Encoder<'static, HashingWriter<BufWriter<File>>>>,
    temporary: PathBuf,
    destination: PathBuf,
    uncompressed_bytes: u64,
}

pub(crate) fn begin_sidecar(index_path: &Path, generation: u64) -> io::Result<SidecarSink> {
    if generation == 0 {
        return Err(crate::compact::invalid(
            "directory summary requires a nonzero generation",
        ));
    }
    let destination = directory_summary_path(index_path);
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = temporary_path(&destination);
    let file = open_private_file(&temporary)?;
    // The header stays outside the checksum: readers hash bytes from
    // PREFIX_BYTES onward, matching the non-streaming writer. Hashing the
    // header here produced sidecars that always failed verification.
    let mut header = BufWriter::new(file);
    header.write_all(MAGIC)?;
    header.write_all(&VERSION.to_le_bytes())?;
    header.write_all(&generation.to_le_bytes())?;
    let compressed = HashingWriter {
        inner: header,
        hasher: crc32fast::Hasher::new(),
    };
    let encoder = zstd::stream::Encoder::new(compressed, 3).map_err(codec)?;
    Ok(SidecarSink {
        encoder: Some(encoder),
        temporary,
        destination,
        uncompressed_bytes: 0,
    })
}

impl SidecarSink {
    pub(crate) fn push(&mut self, entry: DirectorySummaryEntry) -> io::Result<()> {
        let encoded = bincode::serialize(&entry).map_err(codec)?;
        let len = u32::try_from(encoded.len())
            .map_err(|_| crate::compact::invalid("directory summary entry is too large"))?;
        let encoder = self
            .encoder
            .as_mut()
            .ok_or_else(|| crate::compact::invalid("sidecar sink already finished"))?;
        encoder.write_all(&len.to_le_bytes())?;
        encoder.write_all(&encoded)?;
        self.uncompressed_bytes = self
            .uncompressed_bytes
            .checked_add(4 + encoded.len() as u64)
            .ok_or_else(|| crate::compact::invalid("directory summary payload is too large"))?;
        Ok(())
    }

    pub(crate) fn finish(mut self) -> io::Result<()> {
        let encoder = self
            .encoder
            .take()
            .ok_or_else(|| crate::compact::invalid("sidecar sink already finished"))?;
        let compressed = encoder.finish().map_err(codec)?;
        let checksum = compressed.hasher.finalize();
        let mut file = compressed
            .inner
            .into_inner()
            .map_err(|error| error.into_error())?;
        file.write_all(&self.uncompressed_bytes.to_le_bytes())?;
        file.write_all(&checksum.to_le_bytes())?;
        file.sync_all()?;
        drop(file);
        crate::compact_build::replace_file(&self.temporary, &self.destination)?;
        crate::compact_build::sync_parent(&self.destination)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dir_summary::DirectorySummary;
    use crate::{FileKind, FsKind};

    /// The production sidecar path must survive verification, with on-disk
    /// totals intact. The header used to be hashed into the checksum the
    /// readers exclude, so every streamed sidecar failed to open.
    #[test]
    fn streaming_sink_roundtrips_through_verification() {
        let path = std::env::temp_dir().join(format!("neutra-sink-{}.nsx", std::process::id()));
        let _ = std::fs::remove_file(directory_summary_path(&path));
        let mut sink = begin_sidecar(&path, 7).unwrap();
        let mut feed = SummaryFeed::new();
        let record = FileRecord {
            path: "/docs/readme.md".into(),
            size: 42,
            disk: 12,
            mtime: 0,
            mode: 0,
            kind: FileKind::File,
            fs: FsKind::Btrfs,
            native_id: 0,
            native_parent: 0,
            source: 0,
        };
        feed.push(&record, &mut |entry| sink.push(entry)).unwrap();
        feed.finish(&mut |entry| sink.push(entry)).unwrap();
        sink.finish().unwrap();
        let loaded = DirectorySummary::open_for_compact(&path, 7).unwrap();
        let docs = loaded.get(0, "/docs").unwrap();
        assert_eq!(docs.logical_bytes, 42);
        assert_eq!(docs.physical_bytes, 12);
        let _ = std::fs::remove_file(directory_summary_path(&path));
    }
}
