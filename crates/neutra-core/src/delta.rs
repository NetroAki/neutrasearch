//! Bounded mutable overlay for filesystem change events.
//! The immutable compact base is never rewritten per event; changes are first
//! appended to this owner-only WAL and then reflected in a bounded disk index.
use crate::FileRecord;
mod cursor;
mod files;
mod migrate;
mod open;
mod restore;
mod scan;
mod storage;
mod wal;
use cursor::*;
use files::*;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{File, Metadata, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use storage::{Cursor as OverlayCursor, DeltaStore};
use wal::*;

mod format;
pub use format::{DEFAULT_COMPACT_AT, DELTA_HEADER_BYTES};
use format::{HEADER, MAGIC, MAGIC_V1, MAGIC_V2, MAGIC_V3, MAX_FRAME};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DeltaChange {
    Upsert(FileRecord),
    Remove(Box<str>),
    RemovePrefix(Box<str>),
}

pub struct DeltaIndex {
    path: PathBuf,
    generation: u64,
    writer: Option<BufWriter<File>>,
    lock_file: Option<File>,
    overlay: std::sync::Mutex<DeltaStore>,
    wal_bytes: u64,
    compact_at: u64,
    /// True when the on-disk frames use the legacy layout (selected from
    /// the file magic at open). Writers migrate legacy logs on open, so a
    /// live writer implies false.
    wal_v1: bool,
    #[cfg(unix)]
    identity: (u64, u64),
}

impl DeltaIndex {
    pub fn open(path: &Path, generation: u64) -> io::Result<Self> {
        Self::open_mode(path, generation, DEFAULT_COMPACT_AT, true)
    }
    /// Replay a point-in-time WAL snapshot without creating the file or taking
    /// ownership of its single-writer lock.
    pub fn open_snapshot(path: &Path, generation: u64) -> io::Result<Self> {
        Self::open_mode(path, generation, DEFAULT_COMPACT_AT, false)
    }
    pub fn open_with_threshold(path: &Path, generation: u64, compact_at: u64) -> io::Result<Self> {
        Self::open_mode(path, generation, compact_at, true)
    }
    /// Replace an unreadable/torn WAL with an empty generation-bound writer.
    /// Only compaction recovery should call this after verifying that a staged
    /// base already contains the logical overlay.
    pub fn replace_empty(path: &Path, generation: u64) -> io::Result<Self> {
        Self::replace_empty_with_threshold(path, generation, DEFAULT_COMPACT_AT)
    }
    pub fn replace_empty_with_threshold(
        path: &Path,
        generation: u64,
        compact_at: u64,
    ) -> io::Result<Self> {
        if generation == 0 {
            return Err(invalid("delta requires a nonzero base generation"));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let lock_file = open_append_private(&lock_path(path))?;
        lock_file.try_lock_exclusive().map_err(|error| {
            io::Error::new(
                io::ErrorKind::WouldBlock,
                format!("delta log already has a writer: {error}"),
            )
        })?;
        let writer = open_reset_writer(path, generation)?;
        let wal_file = File::open(path)?;
        let identity = file_identity_from_metadata(&wal_file.metadata()?)?;
        let mut overlay = DeltaStore::open(path)?;
        overlay.clear(&overlay_cursor(
            generation,
            HEADER,
            false,
            &wal_file.metadata()?,
        )?)?;
        Ok(Self {
            path: path.to_path_buf(),
            generation,
            writer: Some(writer),
            lock_file: Some(lock_file),
            overlay: std::sync::Mutex::new(overlay),
            wal_bytes: HEADER,
            compact_at: compact_at.max(HEADER + 1),
            wal_v1: false,
            #[cfg(unix)]
            identity,
        })
    }
    /// Tail complete CRC-verified frames appended since this read-only snapshot
    /// was opened. A concurrently written partial final frame remains invisible
    /// until a later refresh.
    pub fn refresh(&mut self) -> io::Result<u64> {
        if self.writer.is_some() {
            return Ok(0);
        }
        if self
            .overlay
            .get_mut()
            .map_err(|_| io::Error::other("delta overlay lock poisoned"))?
            .is_shared()
        {
            let replacement = Self::open_search_snapshot(&self.path, self.generation)?;
            let bytes = replacement.wal_bytes.saturating_sub(self.wal_bytes);
            *self = replacement;
            return Ok(bytes);
        }
        let mut file = File::open(&self.path)?;
        let metadata = file.metadata()?;
        let file_bytes = metadata.len();
        #[cfg(unix)]
        {
            if file_identity_from_metadata(&metadata)? != self.identity {
                return Err(invalid("delta log was checkpointed; reopen the snapshot"));
            }
        }
        // A compaction reset can keep an empty WAL at the same byte length.
        // Validate the generation before the length fast path so persistent
        // readers never continue serving the old mmap base silently.
        let wal_v1 = read_header(&mut file, self.generation)?;
        if wal_v1 != self.wal_v1 {
            return Err(invalid("delta log magic changed; reopen the index"));
        }
        if file_bytes < self.wal_bytes {
            return Err(invalid("delta log was replaced or truncated"));
        }
        if file_bytes == self.wal_bytes {
            return Ok(0);
        }
        file.seek(SeekFrom::Start(self.wal_bytes))?;
        let mut reader = BufReader::new(file.take(file_bytes - self.wal_bytes));
        let mut overlay = self
            .overlay
            .lock()
            .map_err(|_| io::Error::other("delta overlay lock poisoned"))?;
        let old_bytes = self.wal_bytes;
        let mut pending_bytes = old_bytes;
        let result = replay_frames(
            &mut reader,
            &mut pending_bytes,
            &mut overlay,
            self.wal_v1,
            self.generation,
            &metadata,
        );
        if let Err(error) = result {
            overlay.mark_stale();
            return Err(error);
        }
        self.wal_bytes = pending_bytes;
        Ok(self.wal_bytes - old_bytes)
    }
    /// The live record for `path`, when the delta holds one.
    pub fn upsert_for(&self, path: &str) -> io::Result<Option<FileRecord>> {
        self.overlay
            .lock()
            .map_err(|_| io::Error::other("delta overlay lock poisoned"))?
            .get(path)
    }
    pub fn for_each_upsert(
        &self,
        mut f: impl FnMut(FileRecord) -> io::Result<()>,
    ) -> io::Result<()> {
        self.overlay
            .lock()
            .map_err(|_| io::Error::other("delta overlay lock poisoned"))?
            .each(false, |_, record| {
                f(record.ok_or_else(|| invalid("missing upsert record"))?)
            })
    }
    pub fn for_each_removed(
        &self,
        mut f: impl FnMut(Box<str>) -> io::Result<()>,
    ) -> io::Result<()> {
        self.overlay
            .lock()
            .map_err(|_| io::Error::other("delta overlay lock poisoned"))?
            .each(true, |path, _| f(path.into()))
    }
    pub fn for_each_removed_prefix(
        &self,
        mut visit: impl FnMut(Box<str>) -> io::Result<()>,
    ) -> io::Result<()> {
        self.overlay
            .lock()
            .map_err(|_| io::Error::other("delta overlay lock poisoned"))?
            .each_prefix(&mut visit)
    }
    pub fn is_removed(&self, path: &str) -> io::Result<bool> {
        self.overlay
            .lock()
            .map_err(|_| io::Error::other("delta overlay lock poisoned"))?
            .is_removed(path)
    }
    pub fn shadows(&self, path: &str) -> io::Result<bool> {
        self.overlay
            .lock()
            .map_err(|_| io::Error::other("delta overlay lock poisoned"))?
            .shadows(path)
    }
    pub fn shadows_batch(&self, paths: &[&str]) -> io::Result<Vec<bool>> {
        let mut overlay = self
            .overlay
            .lock()
            .map_err(|_| io::Error::other("delta overlay lock poisoned"))?;
        if paths.len() < 16 {
            return overlay.shadows_few(paths);
        }
        let filter = overlay.shadow_filter()?;
        drop(overlay);
        Ok(paths.iter().map(|path| filter.shadows(path)).collect())
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn wal_bytes(&self) -> u64 {
        self.wal_bytes
    }
    pub fn change_count(&self) -> io::Result<usize> {
        self.overlay
            .lock()
            .map_err(|_| io::Error::other("delta overlay lock poisoned"))?
            .count()
    }
    pub fn needs_compaction(&self) -> bool {
        self.wal_bytes >= self.compact_at
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    /// Watcher exclusion roots for a WAL's resolved overlay cache: the
    /// `<wal-parent>/.delta.overlay` directory covers the shared cache,
    /// its `-journal`, and private snapshot files. Watchers must ignore
    /// these paths; they are not source filesystem events.
    pub fn exclusion_roots(wal_path: &Path) -> Vec<PathBuf> {
        DeltaStore::exclusion_roots(wal_path)
    }
}
#[cfg(test)]
mod tests;

mod commit;
#[cfg(target_os = "linux")]
pub use commit::CompactionCursor;

use format::{codec, invalid};
