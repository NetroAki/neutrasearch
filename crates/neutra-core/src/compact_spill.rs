//! Disk spill for bounded-memory index builds: framed record chunks and the
//! ingest accumulator. Sorted runs and k-way merges live in
//! `compact_merge`; the streaming builder consumes them one merge at a time,
//! so a 90M-record machine builds in well under a GiB of RAM.

use crate::compact::{binerr, invalid};
use crate::FileRecord;
use fs2::FileExt;
use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

/// Records per spill chunk: large enough to amortize file churn, small
/// enough to sort comfortably beside a running scan.
const CHUNK_RECORDS: usize = 1_000_000;

/// Length-prefixed record framing so chunk and run files stream record by
/// record on the read side.
pub(crate) fn write_framed(writer: &mut impl Write, record: &FileRecord) -> io::Result<()> {
    let bytes = bincode::serialize(record).map_err(binerr)?;
    let len = u32::try_from(bytes.len()).map_err(|_| invalid("record too large to spill"))?;
    writer.write_all(&len.to_le_bytes())?;
    writer.write_all(&bytes)?;
    Ok(())
}

pub(crate) fn read_framed(reader: &mut impl Read) -> io::Result<Option<FileRecord>> {
    let mut first = [0u8; 1];
    if reader.read(&mut first)? == 0 {
        return Ok(None);
    }
    let mut rest = [0u8; 3];
    reader.read_exact(&mut rest)?;
    let len = u32::from_le_bytes([first[0], rest[0], rest[1], rest[2]]) as usize;
    if len > 64 * 1024 * 1024 {
        return Err(invalid("spill record too large"));
    }
    let mut bytes = vec![0u8; len];
    reader.read_exact(&mut bytes)?;
    bincode::deserialize(&bytes).map_err(binerr).map(Some)
}

/// Ingest side: batches accumulate in RAM up to one chunk, then spill.
pub struct SpillAccumulator {
    dir: Option<PathBuf>,
    chunks: Vec<PathBuf>,
    current: Vec<FileRecord>,
    count: u64,
}

impl SpillAccumulator {
    /// Create the spill directory beside the destination index (same
    /// filesystem, where the space already matters) and sweep any stale
    /// spill from an interrupted build.
    pub fn begin(output: &Path) -> io::Result<Self> {
        if let Some(parent) = output.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let dir = output.with_extension("spill");
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        std::fs::create_dir_all(&dir)?;
        Ok(Self {
            dir: Some(dir),
            chunks: Vec::new(),
            current: Vec::new(),
            count: 0,
        })
    }

    /// Append a scan batch, spilling full chunks. Rejects the same
    /// malformed paths the in-RAM build rejects, and the same record
    /// ceiling.
    pub fn push_batch(&mut self, batch: Vec<FileRecord>) -> io::Result<()> {
        for record in batch {
            if !crate::query::safe_absolute_path(record.path.as_ref()) {
                return Err(invalid(
                    "compact index records must use absolute normalized paths",
                ));
            }
            if self.count == u32::MAX as u64 {
                return Err(invalid("compact index supports at most u32::MAX records"));
            }
            self.current.push(record);
            self.count += 1;
            if self.current.len() >= CHUNK_RECORDS {
                self.spill_chunk()?;
            }
        }
        Ok(())
    }

    fn spill_chunk(&mut self) -> io::Result<()> {
        let Some(dir) = self.dir.as_ref() else {
            return Err(invalid("spill accumulator is closed"));
        };
        let path = dir.join(format!("chunk-{:06}.run", self.chunks.len()));
        let file = File::create(&path)?;
        let mut writer = BufWriter::with_capacity(1 << 20, file);
        for record in self.current.drain(..) {
            write_framed(&mut writer, &record)?;
        }
        writer.flush()?;
        self.chunks.push(path);
        Ok(())
    }

    /// Total records accepted.
    pub fn len(&self) -> u64 {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Spill the remainder and hand over the chunk set for sorting.
    pub fn finish(mut self) -> io::Result<SpillRuns> {
        if !self.current.is_empty() {
            self.spill_chunk()?;
        }
        let Some(dir) = self.dir.take() else {
            return Err(invalid("spill accumulator is closed"));
        };
        Ok(SpillRuns {
            dir: Some(dir),
            chunks: std::mem::take(&mut self.chunks),
            count: self.count,
        })
    }
}

impl Drop for SpillAccumulator {
    fn drop(&mut self) {
        if let Some(dir) = self.dir.take() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// Finished ingest: chunk files plus the record count. Dropping removes
/// the spill directory, so builds clean up on every path.
pub struct SpillRuns {
    dir: Option<PathBuf>,
    chunks: Vec<PathBuf>,
    count: u64,
}

impl SpillRuns {
    /// Total spilled records.
    pub fn len(&self) -> u64 {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub(crate) fn dir(&self) -> io::Result<&Path> {
        self.dir
            .as_deref()
            .ok_or_else(|| invalid("spill runs are closed"))
    }

    pub(crate) fn chunks(&self) -> &[PathBuf] {
        &self.chunks
    }
}

impl Drop for SpillRuns {
    fn drop(&mut self) {
        if let Some(dir) = self.dir.take() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// Serialize the rebuild lock from `CompactIndex::rebuild` so both build
/// paths share one gate and one obsolete-WAL removal. Lives here (rather
/// than beside the builders) because the spill directory it protects is
/// created here.
pub(crate) fn with_rebuild_lock<T>(
    path: &Path,
    build: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    let mut delta = path.to_path_buf();
    delta.set_extension("delta");
    let lock_path = crate::compact::suffix_path(&delta, ".lock");
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let lock = options.open(lock_path)?;
    let metadata = lock.metadata()?;
    if !metadata.is_file() {
        return Err(invalid("compact rebuild lock is not a regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 || metadata.mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "compact rebuild lock must be private and single-linked",
            ));
        }
    }
    match lock.try_lock_exclusive() {
        Ok(()) => {}
        Err(error) => {
            // Windows reports LockFileEx contention as ERROR_LOCK_VIOLATION
            // (33), which fs2 exposes as Uncategorized rather than WouldBlock.
            #[cfg(windows)]
            if error.raw_os_error() == Some(33) {
                return Err(io::Error::new(io::ErrorKind::WouldBlock, error));
            }
            return Err(error);
        }
    }
    let built = build()?;
    match std::fs::remove_file(&delta) {
        Ok(()) => crate::compact_build::sync_parent(&delta)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    Ok(built)
}
