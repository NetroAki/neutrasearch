//! Generation-bound directory summaries derived from native metadata records.
//!
//! This sidecar stores logical folder totals and direct children. It is built
//! from the already-enumerated `FileRecord` stream; it never touches the
//! filesystem tree. The compact search index remains the source of truth for
//! complete records, while this projection lets file-manager views open a
//! folder hierarchy without decoding every compact record.

use crate::{FileKind, FileRecord};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::hash::{Hash, Hasher};
 use std::io::{self, Read, Write};
 #[cfg(test)]
 use std::io::BufWriter;
use std::path::{Path, PathBuf};

pub(crate) const MAGIC: &[u8; 8] = b"NEUDIR01";
 pub(crate) const VERSION: u32 = 2;
 /// Oldest readable sidecar layout. Version 1 entries lack on-disk totals;
 /// they decode through the frozen layout with physical bytes unset.
 pub(crate) const MIN_READABLE_VERSION: u32 = 1;
const PREFIX_BYTES: usize = 20;
const TRAILER_BYTES: usize = 12;
  /// Upper bound for the compressed sidecar file. A 100M-record host writes a
  /// ~1 GiB sidecar, so the cap must admit real files; corrupt input stays
  /// contained by the compressed-bytes checksum plus the stream bound below.
  const MAX_SIDECAR_BYTES: u64 = 2 * 1024 * 1024 * 1024;
 /// Decompressed payload bound. A 100M-record host holds ~20 GiB of
 /// directory entries. Decoding streams without upfront allocation, so the
 /// bound only caps how much a corrupt stream may yield.
 const MAX_UNCOMPRESSED_BYTES: u64 = 32 * 1024 * 1024 * 1024;

#[derive(Debug, Clone)]
pub(crate) struct PathKey {
    pub(crate) source: u32,
    /// Precomputed canonical form; hashing/equality run per lookup, and
    /// canonicalizing on the fly allocates per hash.
    pub(crate) canonical: String,
}

pub(crate) type Key = PathKey;

impl PartialEq for PathKey {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source && self.canonical == other.canonical
    }
}

impl Eq for PathKey {}

impl Hash for PathKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.source.hash(state);
        self.canonical.hash(state);
    }
}

/// One direct child shown by a folder view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectoryChild {
    pub path: Box<str>,
    pub kind: FileKind,
    /// For files this is the file's logical size; for directories it is the
    /// aggregate logical size of the directory subtree.
    pub logical_bytes: u64,
    #[serde(default)]
    pub physical_bytes: u64,
    pub file_count: u64,
    pub directory_count: u64,
}

/// Aggregate metadata for one indexed directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectorySummaryEntry {
    pub source: u32,
    pub path: Box<str>,
    pub logical_bytes: u64,
    #[serde(default)]
    pub physical_bytes: u64,
    pub file_count: u64,
    pub directory_count: u64,
    pub children: Vec<DirectoryChild>,
}

/// Read-only directory summary projection for one compact-index generation.
#[derive(Debug, Clone)]
pub struct DirectorySummary {
    generation: u64,
    entries: Vec<DirectorySummaryEntry>,
}

impl DirectorySummary {
     /// Build and atomically publish the sidecar next to `index_path`.
     /// Test-only: production builds stream through the spill.
     #[cfg(test)]
     pub fn build(records: &[FileRecord], index_path: &Path, generation: u64) -> io::Result<Self> {
        let order = summary_order(records)?;
        write_sidecar_from_order(records, &order, index_path, generation)?;
        Self::open_for_compact(index_path, generation)
    }

     #[cfg(test)]
     pub(crate) fn build_sidecar_ordered(
        records: &[FileRecord],
        order: &[u32],
        index_path: &Path,
        generation: u64,
    ) -> io::Result<()> {
        write_sidecar_from_order(records, order, index_path, generation)
    }

    /// Open the sidecar only when it belongs to the expected compact base.
    pub fn open_for_compact(index_path: &Path, generation: u64) -> io::Result<Self> {
        let (stored_generation, entries) = read_sidecar(&directory_summary_path(index_path))?;
        if stored_generation != generation {
            return Err(invalid(format!(
                "directory summary generation {} does not match compact index generation {}",
                stored_generation, generation
            )));
        }
        Ok(Self {
            generation: stored_generation,
            entries,
        })
    }

    /// Return the sidecar path associated with a compact index.
    pub fn path_for(index_path: &Path) -> PathBuf {
        directory_summary_path(index_path)
    }

    /// Copy a staged sidecar alongside a published compact base. A missing
    /// staged sidecar removes any destination sidecar so stale totals cannot be
    /// mistaken for current data.
    pub fn publish(
        staged_index: &Path,
        destination_index: &Path,
        expected_generation: u64,
    ) -> io::Result<()> {
        let staged = directory_summary_path(staged_index);
        let destination = directory_summary_path(destination_index);
        if staged.is_file() {
            let bytes = read_sidecar_bytes(&staged)?;
            // Validate the generation and compressed checksum before
            // publication; full entry decoding remains the reader's job.
            let generation = sidecar_generation(&bytes)?;
            if generation != expected_generation {
                return Err(invalid("staged directory summary generation mismatch"));
            }
            write_atomic(&destination, &bytes)?;
            std::fs::remove_file(staged)?;
        } else {
            remove_if_present(&destination)?;
        }
        Ok(())
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn entries(&self) -> &[DirectorySummaryEntry] {
        &self.entries
    }

    /// Binary-search the sorted sidecar by source and normalized path.
    pub fn get(&self, source: u32, path: &str) -> Option<&DirectorySummaryEntry> {
        let normalized = normalize_path(path).ok()?;
        self.entries
            .binary_search_by(|entry| compare_key(entry.source, &entry.path, source, &normalized))
            .ok()
            .map(|index| &self.entries[index])
    }

    pub fn children(&self, source: u32, path: &str) -> Option<Vec<DirectoryChild>> {
        let entry = self.get(source, path)?;
        let mut children = entry.children.clone();
        for child in &mut children {
            if child.kind != FileKind::Dir {
                continue;
            }
            if let Some(summary) = self.get(source, &child.path) {
                child.logical_bytes = summary.logical_bytes;
                child.physical_bytes = summary.physical_bytes;
                child.file_count = summary.file_count;
                child.directory_count = summary.directory_count;
            }
        }
        Some(children)
    }

     #[cfg(test)]
     fn from_records(records: &[FileRecord], generation: u64) -> io::Result<Self> {
         let mut entries = aggregate_records(records)?;
         entries.sort_unstable_by(|left, right| {
             compare_key(left.source, &left.path, right.source, &right.path)
         });
         Ok(Self {
             generation,
             entries,
         })
     }
}

fn emit_records<F>(records: &[FileRecord], order: &[u32], mut emit: F) -> io::Result<()>
where
    F: FnMut(DirectorySummaryEntry) -> io::Result<()>,
{
    let mut stack = Vec::<OpenEntry>::new();
    for (position, index) in order.iter().enumerate() {
        let record = &records[*index as usize];
        let normalized_path = normalize_path(record.path.as_ref())?;
        if position > 0 {
            let previous = &records[order[position - 1] as usize];
            if previous.source == record.source
                && compare_paths(previous.path.as_ref(), &normalized_path) == Ordering::Equal
            {
                // Native lanes can expose aliases, but an exact source/path
                // pair must contribute once to prevent duplicate totals.
                continue;
            }
        }
        let ancestors = ancestor_paths(&normalized_path);
        let desired = ancestors
            .get(..ancestors.len().saturating_sub(1))
            .unwrap_or_default();
        let common = stack
            .iter()
            .zip(desired)
            .take_while(|(entry, path)| entry.source == record.source && entry.path == **path)
            .count();
        close_stack(&mut stack, common, &mut emit)?;
        for path in &desired[common..] {
            open_entry(&mut stack, record.source, path);
        }

        if record.kind == FileKind::Dir {
            for entry in &mut stack {
                entry.directory_count = entry.directory_count.saturating_add(1);
            }
            open_entry(&mut stack, record.source, &normalized_path);
            continue;
        }

        let contributes_file = matches!(record.kind, FileKind::File | FileKind::Symlink);
        for entry in &mut stack {
            if contributes_file {
                entry.logical_bytes = entry.logical_bytes.saturating_add(record.size);
                entry.physical_bytes = entry.physical_bytes.saturating_add(record.disk_bytes());
                entry.file_count = entry.file_count.saturating_add(1);
            }
        }
        if let Some(parent) = stack.last_mut() {
            parent.children.push(DirectoryChild {
                path: normalized_path.clone().into_boxed_str(),
                kind: record.kind,
                logical_bytes: record.size,
                physical_bytes: record.disk_bytes(),
                file_count: u64::from(contributes_file),
                directory_count: 0,
            });
        }
    }
    close_stack(&mut stack, 0, &mut emit)
}

pub(crate) struct OpenEntry {
    pub(crate) source: u32,
    pub(crate) path: String,
    pub(crate) logical_bytes: u64,
    pub(crate) physical_bytes: u64,
    pub(crate) file_count: u64,
    pub(crate) directory_count: u64,
    pub(crate) children: Vec<DirectoryChild>,
}

impl OpenEntry {
    fn finish(mut self) -> DirectorySummaryEntry {
        self.children.sort_unstable_by(|left, right| {
            compare_paths(&left.path, &right.path)
                .then_with(|| left.kind_rank().cmp(&right.kind_rank()))
        });
        DirectorySummaryEntry {
            source: self.source,
            path: self.path.into_boxed_str(),
            logical_bytes: self.logical_bytes,
            physical_bytes: self.physical_bytes,
            file_count: self.file_count,
            directory_count: self.directory_count,
            children: self.children,
        }
    }
}

pub(crate) fn open_entry(stack: &mut Vec<OpenEntry>, source: u32, path: &str) {
    if let Some(parent) = stack.last_mut() {
        parent.children.push(DirectoryChild {
            path: path.to_owned().into_boxed_str(),
            kind: FileKind::Dir,
            logical_bytes: 0,
            physical_bytes: 0,
            file_count: 0,
            directory_count: 0,
        });
    }
    stack.push(OpenEntry {
        source,
        path: path.to_owned(),
        logical_bytes: 0,
        physical_bytes: 0,
        file_count: 0,
        directory_count: 0,
        children: Vec::new(),
    });
}

pub(crate) fn close_stack<F>(stack: &mut Vec<OpenEntry>, keep: usize, emit: &mut F) -> io::Result<()>
where
    F: FnMut(DirectorySummaryEntry) -> io::Result<()>,
{
    while stack.len() > keep {
        let finished = stack.pop().expect("stack length checked").finish();
        if let Some(parent) = stack.last_mut() {
            if let Some(child) = parent.children.iter_mut().rev().find(|child| {
                child.kind == FileKind::Dir && child.path.as_ref() == finished.path.as_ref()
            }) {
                child.logical_bytes = finished.logical_bytes;
                child.physical_bytes = finished.physical_bytes;
                child.file_count = finished.file_count;
                child.directory_count = finished.directory_count;
            }
        }
        emit(finished)?;
    }
    Ok(())
}

pub(crate) fn summary_order(records: &[FileRecord]) -> io::Result<Vec<u32>> {
    let mut order = (0..records.len())
        .map(|index| {
            u32::try_from(index).map_err(|_| invalid("too many records for summary order"))
        })
        .collect::<io::Result<Vec<_>>>()?;
    order.par_sort_by(|left, right| {
        records[*left as usize]
            .source
            .cmp(&records[*right as usize].source)
            .then_with(|| {
                compare_paths(
                    records[*left as usize].path.as_ref(),
                    records[*right as usize].path.as_ref(),
                )
            })
    });
    Ok(order)
}

 #[cfg(test)]
 fn write_sidecar_from_order(
    records: &[FileRecord],
    order: &[u32],
    index_path: &Path,
    generation: u64,
) -> io::Result<()> {
    if generation == 0 {
        return Err(invalid("directory summary requires a nonzero generation"));
    }
    let destination = directory_summary_path(index_path);
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = temporary_path(&destination);
    let mut file = open_private_file(&temporary)?;
    file.write_all(MAGIC)?;
    file.write_all(&VERSION.to_le_bytes())?;
    file.write_all(&generation.to_le_bytes())?;
    let mut compressed = HashingWriter {
        inner: BufWriter::new(file),
        hasher: crc32fast::Hasher::new(),
    };
    let mut uncompressed_bytes = 0u64;
    {
        let mut encoder = zstd::stream::Encoder::new(&mut compressed, 3).map_err(codec)?;
        emit_records(records, order, |entry| {
            let encoded = bincode::serialize(&entry).map_err(codec)?;
            let len = u32::try_from(encoded.len())
                .map_err(|_| invalid("directory summary entry is too large"))?;
            encoder.write_all(&len.to_le_bytes())?;
            encoder.write_all(&encoded)?;
            uncompressed_bytes = uncompressed_bytes
                .checked_add(4 + encoded.len() as u64)
                .ok_or_else(|| invalid("directory summary payload is too large"))?;
            Ok(())
        })?;
        encoder.finish().map_err(codec)?;
    }
    let checksum = compressed.hasher.finalize();
    let mut file = compressed
        .inner
        .into_inner()
        .map_err(|error| error.into_error())?;
    file.write_all(&uncompressed_bytes.to_le_bytes())?;
    file.write_all(&checksum.to_le_bytes())?;
    file.sync_all()?;
    drop(file);
    replace_file(&temporary, &destination)?;
    sync_parent(&destination)
}

pub(crate) struct HashingWriter<W> {
    pub(crate) inner: W,
    pub(crate) hasher: crc32fast::Hasher,
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(bytes)?;
        self.hasher.update(&bytes[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}



pub(crate) fn entry_key(source: u32, path: &str) -> Key {
    PathKey {
        source,
        canonical: canonical_path(path),
    }
}

fn compare_key(left_source: u32, left_path: &str, right_source: u32, right_path: &str) -> Ordering {
    left_source
        .cmp(&right_source)
        .then_with(|| compare_paths(left_path, right_path))
}

pub(crate) fn compare_paths(left: &str, right: &str) -> Ordering {
    let left = left.as_bytes();
    let right = right.as_bytes();
    let fold = |byte: u8| {
        let byte = if byte == b'\\' { b'/' } else { byte };
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        {
            byte.to_ascii_lowercase()
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            byte
        }
    };
    for (left, right) in left.iter().zip(right) {
        match fold(*left).cmp(&fold(*right)) {
            Ordering::Equal => {}
            other => return other,
        }
    }
    left.len().cmp(&right.len())
}

pub(crate) fn canonical_path(path: &str) -> String {
    let path = path.replace('\\', "/");
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        path.to_lowercase()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        path
    }
}

pub(crate) fn normalize_path(path: &str) -> io::Result<String> {
    let replaced = path.replace('\\', "/");
    let absolute = replaced.starts_with('/')
        || (replaced.len() >= 3
            && replaced.as_bytes()[0].is_ascii_alphabetic()
            && replaced.as_bytes()[1] == b':'
            && replaced.as_bytes()[2] == b'/');
    if !absolute
        || replaced
            .split('/')
            .any(|component| matches!(component, "." | ".."))
    {
        return Err(invalid(format!("unsafe or relative summary path {path}")));
    }
    let unc = replaced.starts_with("//");
    let mut normalized = replaced
        .split('/')
        .filter(|component| !component.is_empty())
        .collect::<Vec<_>>()
        .join("/");
    if unc {
        normalized.insert_str(0, "//");
    } else if replaced.starts_with('/') {
        normalized.insert(0, '/');
    }
    if replaced.len() >= 3
        && replaced.as_bytes()[0].is_ascii_alphabetic()
        && replaced.as_bytes()[1] == b':'
        && normalized.len() == 2
    {
        normalized.push('/');
    }
    if normalized.is_empty() {
        normalized.push('/');
    }
    Ok(normalized)
}

pub(crate) fn parent_path(path: &str) -> String {
    let normalized = normalize_path(path).unwrap_or_else(|_| path.to_owned());
    if normalized == "/" || is_volume_root(&normalized) {
        return "/".into();
    }
    normalized.rsplit_once('/').map_or_else(
        || "/".into(),
        |(parent, _)| {
            if parent.is_empty() {
                "/".into()
            } else if parent.len() == 2 && parent.ends_with(':') {
                format!("{parent}/")
            } else {
                parent.into()
            }
        },
    )
}

pub(crate) fn ancestor_paths(path: &str) -> Vec<String> {
    let normalized = normalize_path(path).unwrap_or_else(|_| path.to_owned());
    let mut out = vec!["/".to_owned()];
    if is_drive_path(&normalized) {
        let root = normalized[..3].to_owned();
        out.push(root.clone());
        let mut current = root;
        for component in normalized[3..]
            .split('/')
            .filter(|component| !component.is_empty())
        {
            current.push_str(component);
            out.push(current.clone());
            current.push('/');
        }
    } else if let Some(unc) = normalized.strip_prefix("//") {
        let mut components = unc.split('/').filter(|component| !component.is_empty());
        if let (Some(server), Some(share)) = (components.next(), components.next()) {
            let mut current = format!("//{server}/{share}");
            out.push(current.clone());
            for component in components {
                current.push('/');
                current.push_str(component);
                out.push(current.clone());
            }
        }
    } else {
        let mut current = String::new();
        for component in normalized
            .split('/')
            .filter(|component| !component.is_empty())
        {
            current.push('/');
            current.push_str(component);
            out.push(current.clone());
        }
    }
    out.dedup();
    out
}

fn is_drive_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'/'
}

fn is_volume_root(path: &str) -> bool {
    if is_drive_path(path) {
        return path.len() == 3;
    }
    if let Some(unc) = path.strip_prefix("//") {
        return unc.split('/').filter(|part| !part.is_empty()).count() == 2;
    }
    false
}

pub(crate) fn directory_summary_path(index_path: &Path) -> PathBuf {
    let mut value = index_path.as_os_str().to_os_string();
    value.push(".dirs");
    value.into()
}

fn read_sidecar(path: &Path) -> io::Result<(u64, Vec<DirectorySummaryEntry>)> {
    let bytes = read_sidecar_bytes(path)?;
    decode_sidecar(&bytes)
}

fn read_sidecar_bytes(path: &Path) -> io::Result<Vec<u8>> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(invalid("directory summary sidecar is not a regular file"));
    }
    if metadata.len() > MAX_SIDECAR_BYTES {
        return Err(invalid("directory summary sidecar is too large"));
    }
    let mut file = File::open(path)?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn sidecar_generation(bytes: &[u8]) -> io::Result<u64> {
    if bytes.len() < PREFIX_BYTES + TRAILER_BYTES || &bytes[..8] != MAGIC {
        return Err(invalid("not a Neutrasearch directory summary"));
    }
    let version = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    if version != VERSION && version != MIN_READABLE_VERSION {
        return Err(invalid("unsupported directory summary version"));
    }
    let generation = u64::from_le_bytes(bytes[12..20].try_into().unwrap());
    let trailer = bytes.len() - TRAILER_BYTES;
    let uncompressed = u64::from_le_bytes(bytes[trailer..trailer + 8].try_into().unwrap());
    if generation == 0 || uncompressed > MAX_UNCOMPRESSED_BYTES {
        return Err(invalid("invalid directory summary header"));
    }
    let expected_crc = u32::from_le_bytes(bytes[trailer + 8..].try_into().unwrap());
    if crc32fast::hash(&bytes[PREFIX_BYTES..trailer]) != expected_crc {
        return Err(invalid("directory summary checksum mismatch"));
    }
    Ok(generation)
}

 /// Read one sidecar frame in the layout selected by the file version.
 /// Version comes from the sidecar header, never from probing the frame:
 /// old and new frames share a bincode prefix, so sniffing misdecodes.
 fn decode_entry(version: u32, frame: &[u8]) -> bincode::Result<DirectorySummaryEntry> {
     if version == VERSION {
         bincode::deserialize(frame)
     } else {
         Ok(DirectorySummaryEntry::from(
             bincode::deserialize::<OldSummaryEntry>(frame)?,
         ))
     }
 }

/// Frozen pre-physical entry layout, for sidecars written by older binaries.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct OldSummaryEntry {
    source: u32,
    path: Box<str>,
    logical_bytes: u64,
    file_count: u64,
    directory_count: u64,
    children: Vec<OldSummaryChild>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OldSummaryChild {
    path: Box<str>,
    kind: FileKind,
    logical_bytes: u64,
    file_count: u64,
    directory_count: u64,
}

impl From<OldSummaryEntry> for DirectorySummaryEntry {
    fn from(old: OldSummaryEntry) -> Self {
        Self {
            source: old.source,
            path: old.path,
            logical_bytes: old.logical_bytes,
            physical_bytes: 0,
            file_count: old.file_count,
            directory_count: old.directory_count,
            children: old.children.into_iter().map(DirectoryChild::from).collect(),
        }
    }
}

impl From<OldSummaryChild> for DirectoryChild {
    fn from(old: OldSummaryChild) -> Self {
        Self {
            path: old.path,
            kind: old.kind,
            logical_bytes: old.logical_bytes,
            physical_bytes: 0,
            file_count: old.file_count,
            directory_count: old.directory_count,
        }
    }
}

 /// Aggregate records into summary entries in file order, unsorted. The tree
 /// builds straight from these; callers needing binary search sort them.
 pub fn aggregate_records(
     records: &[FileRecord],
 ) -> io::Result<Vec<DirectorySummaryEntry>> {
     let order = summary_order(records)?;
     let mut entries = Vec::new();
     emit_records(records, &order, |entry| {
         entries.push(entry);
         Ok(())
     })?;
     Ok(entries)
 }

 /// Walk validated frames in file order, emitting each entry without
 /// retaining the payload. Skips the entry sort and cross-entry validation
 /// of decode_sidecar: consumers that need binary search or parent/child
 /// consistency use decode_sidecar. Returns the file layout version and
 /// base generation from the header.
 pub(crate) fn walk_frames(
     bytes: &[u8],
     emit: &mut impl FnMut(DirectorySummaryEntry) -> io::Result<()>,
 ) -> io::Result<(u32, u64)> {
     if bytes.len() < PREFIX_BYTES + TRAILER_BYTES || &bytes[..8] != MAGIC {
         return Err(invalid("not a Neutrasearch directory summary"));
     }
     let version = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
     if version != VERSION && version != MIN_READABLE_VERSION {
         return Err(invalid("unsupported directory summary version"));
     }
     let generation = u64::from_le_bytes(bytes[12..20].try_into().unwrap());
     let trailer = bytes.len() - TRAILER_BYTES;
     let uncompressed = u64::from_le_bytes(bytes[trailer..trailer + 8].try_into().unwrap());
     if generation == 0 || uncompressed > MAX_UNCOMPRESSED_BYTES {
         return Err(invalid("invalid directory summary header"));
     }
     let expected_crc = u32::from_le_bytes(bytes[trailer + 8..].try_into().unwrap());
     let compressed = &bytes[PREFIX_BYTES..trailer];
     if crc32fast::hash(compressed) != expected_crc {
         return Err(invalid("directory summary checksum mismatch"));
     }
     // Parse length-prefixed frames incrementally: buffering the whole
     // payload would pin ~20 GiB on a 100M-record host. Only complete
     // frames plus one read chunk are retained.
     let decoder = zstd::stream::Decoder::new(compressed).map_err(codec)?;
     let mut decoder = decoder;
     let mut buf = Vec::new();
     let mut chunk = vec![0u8; 1024 * 1024];
     loop {
         let mut cursor = 0usize;
         loop {
             if buf.len() - cursor < 4 {
                 break;
             }
             let length = u32::from_le_bytes(
                 buf[cursor..cursor + 4].try_into().unwrap(),
             ) as usize;
             let frame_end = cursor
                 .checked_add(4)
                 .and_then(|start| start.checked_add(length))
                 .ok_or_else(|| invalid("directory summary frame length overflow"))?;
             if frame_end > buf.len() {
                 break;
             }
             emit(decode_entry(version, &buf[cursor + 4..frame_end]).map_err(codec)?)?;
             cursor = frame_end;
         }
         buf.drain(..cursor);
         let read = decoder.read(&mut chunk).map_err(codec)?;
         if read == 0 {
             if !buf.is_empty() {
                 return Err(invalid("truncated directory summary frame"));
             }
             break;
         }
         buf.extend_from_slice(&chunk[..read]);
         if buf.len() as u64 > MAX_UNCOMPRESSED_BYTES + 1 {
             return Err(invalid("directory summary payload exceeds safety cap"));
         }
     }
     Ok((version, generation))
 }

 fn decode_sidecar(bytes: &[u8]) -> io::Result<(u64, Vec<DirectorySummaryEntry>)> {
     let mut entries = Vec::new();
     let (_, generation) = walk_frames(bytes, &mut |entry| {
         entries.push(entry);
         Ok(())
     })?;
     validate_entries(&entries)?;
    entries.sort_unstable_by(|left, right| {
        compare_key(left.source, &left.path, right.source, &right.path)
    });
    Ok((generation, entries))
}

fn validate_entries(entries: &[DirectorySummaryEntry]) -> io::Result<()> {
    let keys = entries
        .iter()
        .map(|entry| entry_key(entry.source, &entry.path))
        .collect::<HashSet<_>>();
    for entry in entries {
        if normalize_path(&entry.path)? != entry.path.as_ref() {
            return Err(invalid("directory summary contains an unnormalized path"));
        }
        for child in &entry.children {
            if normalize_path(&child.path)? != child.path.as_ref() {
                return Err(invalid(
                    "directory summary contains an unnormalized child path",
                ));
            }
            if parent_path(&child.path) != entry.path.as_ref() {
                return Err(invalid("directory summary child has the wrong parent"));
            }
            if child.kind == FileKind::Dir && !keys.contains(&entry_key(entry.source, &child.path))
            {
                return Err(invalid("directory summary child directory is missing"));
            }
        }
    }
    Ok(())
}

pub(crate) fn open_private_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true).read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options.open(path)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = temporary_path(path);
    let mut file = open_private_file(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    replace_file(&temporary, path)?;
    sync_parent(path)
}

pub(crate) fn temporary_path(path: &Path) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut value = path.as_os_str().to_os_string();
    value.push(format!(".new-{}-{nonce}", std::process::id()));
    value.into()
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => sync_parent(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(not(windows))]
fn replace_file(temporary: &Path, destination: &Path) -> io::Result<()> {
    std::fs::rename(temporary, destination)
}

#[cfg(windows)]
fn replace_file(temporary: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    #[link(name = "kernel32")]
    extern "system" {
        fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
    }
    let existing = temporary
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let replacement = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        MoveFileExW(
            existing.as_ptr(),
            replacement.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn sync_parent(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn sync_parent(_path: &Path) -> io::Result<()> {
    Ok(())
}

pub(crate) fn codec(error: impl std::fmt::Display) -> io::Error {
    invalid(format!("directory summary codec: {error}"))
}

pub(crate) fn invalid(error: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.into())
}

pub(crate) trait ChildKindRank {
    fn kind_rank(&self) -> u8;
}

impl ChildKindRank for DirectoryChild {
    fn kind_rank(&self) -> u8 {
        u8::from(self.kind != FileKind::Dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CompactIndex;
    use crate::FsKind;

    fn record(path: &str, size: u64, kind: FileKind) -> FileRecord {
        FileRecord {
            path: path.into(),
            size,
            mtime: 0,
            mode: 0,
            kind,
            fs: FsKind::Btrfs,
            native_id: size + 100,
            native_parent: 0,
            source: 0,
            disk: 0,
        }
    }

    fn cleanup(path: &Path) {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(directory_summary_path(path));
    }

    #[test]
    fn entry_decode_follows_the_file_version() {
        let current = DirectorySummaryEntry {
            source: 0,
            path: "/".into(),
            logical_bytes: 10,
            physical_bytes: 4,
            file_count: 1,
            directory_count: 0,
            children: Vec::new(),
        };
        let frame = bincode::serialize(&current).unwrap();
        assert_eq!(decode_entry(VERSION, &frame).unwrap().physical_bytes, 4);
        let legacy = OldSummaryEntry {
            source: 0,
            path: "/".into(),
            logical_bytes: 10,
            file_count: 1,
            directory_count: 0,
            children: Vec::new(),
        };
        let frame = bincode::serialize(&legacy).unwrap();
        let decoded = decode_entry(MIN_READABLE_VERSION, &frame).unwrap();
        assert_eq!(decoded.logical_bytes, 10);
        assert_eq!(decoded.physical_bytes, 0);
    }

    #[test]
    fn aggregates_nested_logical_sizes_and_children() {
        let records = vec![
            record("/home/a/one.txt", 10, FileKind::File),
            record("/home/a/two.txt", 20, FileKind::File),
            record("/home/a/nested", 0, FileKind::Dir),
            record("/home/a/nested/three.txt", 30, FileKind::File),
        ];
        let summary = DirectorySummary::from_records(&records, 7).unwrap();
        let home = summary.get(0, "/home").unwrap();
        assert_eq!((home.logical_bytes, home.file_count), (60, 3));
        let folder = summary.get(0, "/home/a").unwrap();
        assert_eq!((folder.logical_bytes, folder.file_count), (60, 3));
        assert_eq!(folder.directory_count, 1);
        let nested = summary
            .children(0, "/home/a")
            .unwrap()
            .into_iter()
            .find(|child| child.path.as_ref() == "/home/a/nested")
            .unwrap();
        assert_eq!((nested.logical_bytes, nested.file_count), (30, 1));
    }

    #[test]
    fn duplicate_source_path_contributes_once() {
        let records = vec![
            record("/same.txt", 10, FileKind::File),
            record("/same.txt", 99, FileKind::File),
        ];
        let summary = DirectorySummary::from_records(&records, 1).unwrap();
        assert_eq!(summary.get(0, "/").unwrap().logical_bytes, 10);
    }

    #[test]
    fn sidecar_roundtrip_is_generation_bound() {
        let path =
            std::env::temp_dir().join(format!("neutra-dir-summary-{}.nsx", std::process::id()));
        cleanup(&path);
        let records = vec![record("/docs/readme.md", 42, FileKind::File)];
        DirectorySummary::build(&records, &path, 42).unwrap();
        let loaded = DirectorySummary::open_for_compact(&path, 42).unwrap();
        assert_eq!(loaded.get(0, "/docs").unwrap().logical_bytes, 42);
        assert!(DirectorySummary::open_for_compact(&path, 41).is_err());
        cleanup(&path);
    }

    #[test]
    fn normalizes_windows_and_unc_paths() {
        let records = vec![
            record(r"C:\Users\Alex\report.txt", 4, FileKind::File),
            record(r"\\server\share\team\plan.txt", 5, FileKind::File),
        ];
        let summary = DirectorySummary::from_records(&records, 3).unwrap();
        assert!(summary.get(0, "C:/Users/Alex").is_some());
        assert!(summary.get(0, "//server/share/team").is_some());
    }

    #[test]
    fn publish_moves_the_generation_bound_sidecar_with_the_base() {
        let base_path =
            std::env::temp_dir().join(format!("neutra-dir-publish-{}.nsx", std::process::id()));
        let staged_path =
            std::env::temp_dir().join(format!("neutra-dir-publish-{}.compact", std::process::id()));
        cleanup(&base_path);
        cleanup(&staged_path);
        let old_records = vec![record("/old.txt", 1, FileKind::File)];
        let new_records = vec![record("/new.txt", 2, FileKind::File)];
        CompactIndex::build_with_summary(&old_records, &base_path).unwrap();
        let staged = CompactIndex::build_with_summary(&new_records, &staged_path).unwrap();
        CompactIndex::publish(&staged_path, &base_path).unwrap();
        let summary = DirectorySummary::open_for_compact(&base_path, staged.generation).unwrap();
        assert!(summary
            .get(0, "/")
            .unwrap()
            .children
            .iter()
            .any(|child| child.path.as_ref() == "/new.txt"));
        assert!(!DirectorySummary::path_for(&staged_path).exists());
        cleanup(&base_path);
        cleanup(&staged_path);
    }
}
