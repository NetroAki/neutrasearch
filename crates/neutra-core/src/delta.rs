//! Bounded mutable overlay for filesystem change events.
//! The immutable compact base is never rewritten per event; changes are first
//! appended to this owner-only WAL and then reflected in a bounded disk index.
use crate::FileRecord;
mod storage;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{File, Metadata, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use storage::{Cursor as OverlayCursor, DeltaStore};

const MAGIC: &[u8; 8] = b"NEUTDLT3";
const MAGIC_V2: &[u8; 8] = b"NEUTDLT2";
/// Magic written by binaries predating on-disk sizes. Logs under this magic
/// carry the frozen frame layout; replay selects the decoder from the file
/// magic, never by probing frames.
const MAGIC_V1: &[u8; 8] = b"NEUTDLT1";
pub const DELTA_HEADER_BYTES: u64 = 16;
const HEADER: u64 = DELTA_HEADER_BYTES;
const MAX_FRAME: usize = 16 * 1024 * 1024;
pub const DEFAULT_COMPACT_AT: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DeltaChange {
    Upsert(FileRecord),
    Remove(Box<str>),
}

/// Frozen pre-disk WAL frame, for logs written by older binaries.
#[derive(Debug, Clone, Serialize, Deserialize)]
enum OldDeltaChange {
    Upsert(OldDeltaRecord),
    Remove(Box<str>),
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct OldDeltaRecord {
    path: Box<str>,
    size: u64,
    mtime: i64,
    mode: u32,
    kind: crate::FileKind,
    fs: crate::mounts::FsKind,
    #[serde(default)]
    native_id: u64,
    #[serde(default)]
    native_parent: u64,
    source: u32,
}
/// Decode one WAL frame in the layout selected by the log magic. Version
/// comes from the file header, never from probing the frame.
fn decode_change(v1: bool, payload: &[u8]) -> Result<DeltaChange, bincode::Error> {
    if !v1 {
        return bincode::deserialize(payload);
    }
    match bincode::deserialize::<OldDeltaChange>(payload)? {
        OldDeltaChange::Remove(path) => Ok(DeltaChange::Remove(path)),
        OldDeltaChange::Upsert(old) => Ok(DeltaChange::Upsert(FileRecord {
            path: old.path,
            size: old.size,
            disk: 0,
            mtime: old.mtime,
            mode: old.mode,
            kind: old.kind,
            fs: old.fs,
            native_id: old.native_id,
            native_parent: old.native_parent,
            source: old.source,
        })),
    }
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

impl Drop for DeltaIndex {
    fn drop(&mut self) {
        // Explicitly release BSD flock locks before closing the descriptor.
        // macOS can otherwise briefly report the just-dropped writer as busy
        // when the same process immediately reopens it.
        if let Some(lock_file) = self.lock_file.take() {
            let _ = FileExt::unlock(&lock_file);
        }
    }
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
    fn open_mode(
        path: &Path,
        generation: u64,
        compact_at: u64,
        writable: bool,
    ) -> io::Result<Self> {
        if generation == 0 {
            return Err(invalid("delta requires a nonzero base generation"));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Lock a stable sibling rather than the WAL itself. Windows byte-range
        // locks are mandatory and would otherwise prevent read-only snapshots.
        let lock_file = if writable {
            let file = open_append_private(&lock_path(path))?;
            file.try_lock_exclusive().map_err(|error| {
                io::Error::new(
                    io::ErrorKind::WouldBlock,
                    format!("delta log already has a writer: {error}"),
                )
            })?;
            Some(file)
        } else {
            None
        };
        let wal_file = match File::open(path) {
            Ok(file) => Some(file),
            Err(error) if writable && error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        let metadata = wal_file.as_ref().map(File::metadata).transpose()?;
        let file_bytes = metadata.as_ref().map_or(0, Metadata::len);
        let mut overlay = if writable {
            DeltaStore::open(path)?
        } else {
            DeltaStore::open_private(path)?
        };
        if !writable || file_bytes == 0 {
            overlay.clear_unknown()?;
        }
        let mut next_compact_at = compact_at.max(HEADER + 1);
        let mut wal_bytes = file_bytes;
        let mut wal_v1 = false;
        #[cfg(unix)]
        let mut identity = None;
        if file_bytes > 0 {
            let file = wal_file
                .as_ref()
                .expect("WAL opened for nonzero length")
                .try_clone()?;
            let metadata = file.metadata()?;
            #[cfg(unix)]
            {
                identity = Some(file_identity_from_metadata(&metadata)?);
            }
            let mut reader = BufReader::new(file.take(file_bytes));
            wal_v1 = read_header(&mut reader, generation)?;
            let current = overlay_cursor(generation, HEADER, wal_v1, &metadata)?;
            let cached = if writable { overlay.cursor()? } else { None };
            let valid = writable
                && cached.is_some_and(|c| {
                    c.generation == generation
                        && c.wal_bytes >= HEADER
                        && c.wal_bytes <= file_bytes
                        && c.legacy_frames == wal_v1
                        && same_file_identity(c, current)
                        && (c.wal_bytes < file_bytes || c.modified_ns == current.modified_ns)
                });
            let mut verified_bytes = HEADER;
            if valid {
                if compact_at == DEFAULT_COMPACT_AT {
                    if let Some(bytes) = overlay.checkpoint_bytes()? {
                        next_compact_at = next_compact_at.max(bytes.saturating_mul(2));
                    }
                }
                verified_bytes = cached.expect("validated cursor").wal_bytes;
                // The persistent overlay avoids SQL replay, but it never
                // substitutes for validating the WAL's authoritative bytes.
                // Stream frame CRCs without decoding payloads or allocating
                // per-record state before trusting a cached prefix.
                verify_frame_prefix(&mut reader, verified_bytes)?;
                reader.seek(SeekFrom::Start(verified_bytes))?;
                replay_frames(
                    &mut reader,
                    &mut verified_bytes,
                    &mut overlay,
                    wal_v1,
                    generation,
                    &metadata,
                )?;
            } else {
                overlay.clear(&current)?;
                reader.seek(SeekFrom::Start(HEADER))?;
                replay_frames(
                    &mut reader,
                    &mut verified_bytes,
                    &mut overlay,
                    wal_v1,
                    generation,
                    &metadata,
                )?;
            }
            wal_bytes = verified_bytes;
            if writable {
                overlay.set_cursor(&overlay_cursor(
                    generation,
                    verified_bytes,
                    wal_v1,
                    &metadata,
                )?)?;
            }
        }
        // A writer never appends to a v1 log. Migrating on open keeps one
        // frame layout per file; the replayed maps already hold the resolved
        // state, so re-appending them is order-safe.
        let migrated = writable && wal_v1;
        let writer = if writable {
            if migrated {
                // Keep the verified legacy WAL intact until the complete new
                // representation has been synced and atomically published.
                None
            } else {
                if file_bytes > wal_bytes {
                    truncate_private(path, wal_bytes)?;
                }
                let mut file = open_append_private(path)?;
                if wal_bytes == 0 {
                    file.write_all(MAGIC)?;
                    file.write_all(&generation.to_le_bytes())?;
                    file.sync_data()?;
                    wal_bytes = HEADER;
                }
                Some(BufWriter::with_capacity(64 * 1024, file))
            }
        } else {
            None
        };
        let mut this = Self {
            path: path.to_path_buf(),
            generation,
            writer,
            lock_file,
            overlay: std::sync::Mutex::new(overlay),
            wal_bytes,
            compact_at: next_compact_at,
            wal_v1,
            #[cfg(unix)]
            identity: match identity {
                Some(identity) => identity,
                None => file_identity(path)?,
            },
        };
        if migrated {
            // Keep the old WAL authoritative until the replacement is synced.
            let staged = crate::compact::suffix_path(path, ".migrate");
            let mut rewritten = open_reset_writer(&staged, generation)?;
            let mut bytes = 0u64;
            let mut since_flush = 0usize;
            {
                let mut append = |change: DeltaChange| -> io::Result<()> {
                    bytes += append_frame(&mut rewritten, &change)?;
                    since_flush += 1;
                    if since_flush >= 4096 {
                        rewritten.flush()?;
                        since_flush = 0;
                    }
                    Ok(())
                };
                this.for_each_removed(|path| append(DeltaChange::Remove(path)))?;
                this.for_each_upsert(|record| append(DeltaChange::Upsert(record)))?;
            }
            rewritten.flush()?;
            rewritten.get_ref().sync_all()?;
            drop(rewritten);
            crate::compact_build::replace_file(&staged, path)?;
            crate::compact_build::sync_parent(path)?;
            let rewritten = open_append_private(path)?;
            #[cfg(unix)]
            {
                this.identity = file_identity_from_metadata(&rewritten.metadata()?)?;
            }
            let cursor = overlay_cursor(generation, HEADER + bytes, false, &rewritten.metadata()?)?;
            this.overlay
                .get_mut()
                .map_err(|_| io::Error::other("delta overlay lock poisoned"))?
                .set_cursor(&cursor)?;
            this.writer = Some(BufWriter::with_capacity(64 * 1024, rewritten));
            this.wal_bytes = HEADER + bytes;
            this.wal_v1 = false;
        }
        Ok(this)
    }
    pub fn apply(&mut self, change: DeltaChange) -> io::Result<()> {
        self.apply_batch(std::iter::once(change))?;
        Ok(())
    }

    /// Append a batch of changes with one writer flush at the end. Event
    /// watchers deliver bursts; flushing per frame would cost one write
    /// syscall per change.
    pub fn apply_batch<I: IntoIterator<Item = DeltaChange>>(
        &mut self,
        changes: I,
    ) -> io::Result<u32> {
        if self.writer.is_none() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "read-only delta snapshot",
            ));
        }
        let mut pending = Vec::new();
        let mut pending_bytes = self.wal_bytes;
        let mut count = 0u32;
        for change in changes {
            let payload_size = bincode::serialized_size(&change).map_err(codec)? as usize;
            if payload_size > MAX_FRAME {
                return Err(invalid("delta change exceeds safety cap"));
            }
            if pending.len() == 4096 {
                self.flush_overlay_batch(&pending, pending_bytes)?;
                pending.clear();
                self.wal_bytes = pending_bytes;
            }
            let writer = self.writer.as_mut().expect("writer checked");
            pending_bytes = pending_bytes.saturating_add(append_frame(writer, &change)?);
            pending.push(change);
            count = count.saturating_add(1);
        }
        self.writer.as_mut().ok_or_else(|| {
            io::Error::new(io::ErrorKind::PermissionDenied, "read-only delta snapshot")
        })?;
        if !pending.is_empty() {
            self.flush_overlay_batch(&pending, pending_bytes)?;
            self.wal_bytes = pending_bytes;
        }
        Ok(count)
    }

    fn flush_overlay_batch(&mut self, pending: &[DeltaChange], end_bytes: u64) -> io::Result<()> {
        let writer = self.writer.as_mut().ok_or_else(|| {
            io::Error::new(io::ErrorKind::PermissionDenied, "read-only delta snapshot")
        })?;
        writer.flush()?;
        writer.get_ref().sync_data()?;
        let metadata = writer.get_ref().metadata()?;
        let cursor = overlay_cursor(self.generation, end_bytes, self.wal_v1, &metadata)?;
        let overlay = self
            .overlay
            .get_mut()
            .map_err(|_| io::Error::other("delta overlay lock poisoned"))?;
        if let Err(error) = overlay.apply_batch(pending, &cursor) {
            overlay.mark_stale();
            return Err(error);
        }
        Ok(())
    }
    pub fn sync(&mut self) -> io::Result<()> {
        let writer = self.writer.as_mut().ok_or_else(|| {
            io::Error::new(io::ErrorKind::PermissionDenied, "read-only delta snapshot")
        })?;
        writer.flush()?;
        writer.get_ref().sync_data()
    }

    /// Collapse repeated changes without decoding or rebuilding the immutable
    /// base. The stable writer lock covers the atomic log replacement.
    #[cfg(unix)]
    pub fn checkpoint(&mut self) -> io::Result<()> {
        self.sync()?;
        let staged = crate::compact::suffix_path(&self.path, ".checkpoint");
        match std::fs::remove_file(&staged) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let mut writer = open_reset_writer(&staged, self.generation)?;
        let mut bytes = HEADER;
        self.for_each_removed(|path| {
            bytes += append_frame(&mut writer, &DeltaChange::Remove(path))?;
            Ok(())
        })?;
        self.for_each_upsert(|record| {
            bytes += append_frame(&mut writer, &DeltaChange::Upsert(record))?;
            Ok(())
        })?;
        writer.flush()?;
        writer.get_ref().sync_all()?;
        drop(writer);
        crate::compact_build::replace_file(&staged, &self.path)?;
        self.writer = Some(BufWriter::with_capacity(
            64 * 1024,
            open_append_private(&self.path)?,
        ));
        let wal_file = File::open(&self.path)?;
        let wal_metadata = wal_file.metadata()?;
        self.identity = file_identity_from_metadata(&wal_metadata)?;
        self.wal_bytes = bytes;
        self.overlay
            .get_mut()
            .map_err(|_| io::Error::other("delta overlay lock poisoned"))?
            .set_checkpoint_cursor(&overlay_cursor(
                self.generation,
                self.wal_bytes,
                self.wal_v1,
                &wal_metadata,
            )?)?;
        self.compact_at = DEFAULT_COMPACT_AT.max(bytes.saturating_mul(2));
        crate::compact_build::sync_parent(&self.path)
    }
    /// Reset this writer to an empty WAL for a replacement base generation.
    /// The stable writer lock remains held throughout the transition.
    pub fn reset(&mut self, generation: u64) -> io::Result<()> {
        if generation == 0 {
            return Err(invalid("delta requires a nonzero base generation"));
        }
        let mut old_writer = self.writer.take().ok_or_else(|| {
            io::Error::new(io::ErrorKind::PermissionDenied, "read-only delta snapshot")
        })?;
        old_writer.flush()?;
        old_writer.get_ref().sync_data()?;
        drop(old_writer);
        // Windows cannot truncate through an append-mode handle. Keep the
        // sibling writer lock, close append, truncate with a dedicated handle,
        // then reopen append for subsequent frames.
        let writer = open_reset_writer(&self.path, generation)?;
        let wal_metadata = writer.get_ref().metadata()?;
        self.writer = Some(writer);
        self.generation = generation;
        self.wal_bytes = HEADER;
        self.overlay
            .get_mut()
            .map_err(|_| io::Error::other("delta overlay lock poisoned"))?
            .clear(&overlay_cursor(generation, HEADER, false, &wal_metadata)?)?;
        Ok(())
    }
    /// Tail complete CRC-verified frames appended since this read-only snapshot
    /// was opened. A concurrently written partial final frame remains invisible
    /// until a later refresh.
    pub fn refresh(&mut self) -> io::Result<u64> {
        if self.writer.is_some() {
            return Ok(0);
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
        let overlay = self
            .overlay
            .lock()
            .map_err(|_| io::Error::other("delta overlay lock poisoned"))?;
        overlay.shadows_many(paths)
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
/// Validate the log header against the expected base generation and report
/// whether frames use the legacy layout. Frame decoders are selected from
/// this file magic, never by probing frame bytes.
fn read_header(reader: &mut impl Read, generation: u64) -> io::Result<bool> {
    let mut magic = [0u8; 8];
    reader.read_exact(&mut magic)?;
    let wal_v1 = if &magic == MAGIC || &magic == MAGIC_V2 {
        false
    } else if &magic == MAGIC_V1 {
        true
    } else {
        return Err(invalid("not a Neutrasearch delta log"));
    };
    let mut stored_generation = [0u8; 8];
    reader.read_exact(&mut stored_generation)?;
    if u64::from_le_bytes(stored_generation) != generation {
        return Err(invalid("delta log belongs to a different base generation"));
    }
    Ok(wal_v1)
}

#[cfg(unix)]
fn file_identity_from_metadata(metadata: &Metadata) -> io::Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(unix)]
fn file_identity(path: &Path) -> io::Result<(u64, u64)> {
    file_identity_from_metadata(&File::open(path)?.metadata()?)
}

#[cfg(not(unix))]
fn file_identity_from_metadata(_: &Metadata) -> io::Result<(u64, u64)> {
    Ok((0, 0))
}

fn append_frame(writer: &mut impl Write, change: &DeltaChange) -> io::Result<u64> {
    let payload = bincode::serialize(change).map_err(codec)?;
    if payload.len() > MAX_FRAME {
        return Err(invalid("delta change exceeds safety cap"));
    }
    writer.write_all(&(payload.len() as u32).to_le_bytes())?;
    writer.write_all(&crc32fast::hash(&payload).to_le_bytes())?;
    writer.write_all(&payload)?;
    Ok(payload.len() as u64 + 8)
}

fn verify_frame_prefix(reader: &mut (impl Read + Seek), end: u64) -> io::Result<()> {
    if end < HEADER {
        return Err(invalid("delta cache cursor precedes WAL header"));
    }
    reader.seek(SeekFrom::Start(HEADER))?;
    let mut verified = HEADER;
    let mut scratch = [0u8; 64 * 1024];
    while verified < end {
        if end - verified < 8 {
            return Err(invalid("delta cache cursor falls inside a frame header"));
        }
        let mut frame_header = [0u8; 8];
        reader.read_exact(&mut frame_header)?;
        let len = u32::from_le_bytes(frame_header[..4].try_into().expect("fixed slice")) as usize;
        let expected = u32::from_le_bytes(frame_header[4..].try_into().expect("fixed slice"));
        if len > MAX_FRAME || len as u64 > end - verified - 8 {
            return Err(invalid("delta cache cursor falls inside a frame"));
        }
        let mut remaining = len;
        let mut crc = crc32fast::Hasher::new();
        while remaining > 0 {
            let take = remaining.min(scratch.len());
            reader.read_exact(&mut scratch[..take])?;
            crc.update(&scratch[..take]);
            remaining -= take;
        }
        if crc.finalize() != expected {
            return Err(invalid("delta frame checksum mismatch"));
        }
        verified += 8 + len as u64;
    }
    if verified != end {
        return Err(invalid("delta cache cursor is not on a frame boundary"));
    }
    Ok(())
}

fn replay_frames(
    reader: &mut impl Read,
    verified_bytes: &mut u64,
    overlay: &mut DeltaStore,
    wal_v1: bool,
    generation: u64,
    metadata: &Metadata,
) -> io::Result<()> {
    const REPLAY_BATCH: usize = 4096;
    let mut changes = Vec::with_capacity(REPLAY_BATCH);
    loop {
        let mut len = [0u8; 4];
        match reader.read_exact(&mut len) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(error),
        }
        let len = u32::from_le_bytes(len) as usize;
        if len > MAX_FRAME {
            return Err(invalid("delta frame exceeds safety cap"));
        }
        let mut expected_crc = [0u8; 4];
        match reader.read_exact(&mut expected_crc) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(error),
        }
        let mut payload = vec![0; len];
        match reader.read_exact(&mut payload) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(error),
        }
        let frame_end = *verified_bytes + 8 + len as u64;
        if crc32fast::hash(&payload) != u32::from_le_bytes(expected_crc) {
            return Err(invalid("delta frame checksum mismatch"));
        }
        let change: DeltaChange = decode_change(wal_v1, &payload).map_err(codec)?;
        changes.push(change);
        *verified_bytes = frame_end;
        if changes.len() == REPLAY_BATCH {
            let cursor = overlay_cursor(generation, *verified_bytes, wal_v1, metadata)?;
            overlay.apply_batch(&changes, &cursor)?;
            changes.clear();
        }
    }
    if !changes.is_empty() {
        let cursor = overlay_cursor(generation, *verified_bytes, wal_v1, metadata)?;
        overlay.apply_batch(&changes, &cursor)?;
    }
    Ok(())
}

fn overlay_cursor(
    generation: u64,
    wal_bytes: u64,
    legacy_frames: bool,
    metadata: &Metadata,
) -> io::Result<OverlayCursor> {
    #[cfg(unix)]
    let (dev, ino) = file_identity_from_metadata(metadata)?;
    let modified_ns = metadata
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();
    Ok(OverlayCursor {
        generation,
        wal_bytes,
        legacy_frames,
        modified_ns,
        #[cfg(unix)]
        dev,
        #[cfg(unix)]
        ino,
    })
}

#[cfg(unix)]
fn same_file_identity(a: OverlayCursor, b: OverlayCursor) -> bool {
    a.dev == b.dev && a.ino == b.ino
}
#[cfg(not(unix))]
fn same_file_identity(_: OverlayCursor, _: OverlayCursor) -> bool {
    true
}

fn lock_path(path: &Path) -> PathBuf {
    let mut lock = path.as_os_str().to_os_string();
    lock.push(".lock");
    lock.into()
}
fn truncate_private(path: &Path, len: u64) -> io::Result<()> {
    reject_symlink(path)?;
    let mut options = OpenOptions::new();
    options.create(true).write(true).truncate(false);
    configure_private_options(&mut options);
    let file = options.open(path)?;
    validate_private_file(&file)?;
    file.set_len(len)?;
    file.sync_data()
}
fn open_reset_writer(path: &Path, generation: u64) -> io::Result<BufWriter<File>> {
    truncate_private(path, 0)?;
    let mut file = open_append_private(path)?;
    file.write_all(MAGIC)?;
    file.write_all(&generation.to_le_bytes())?;
    file.sync_all()?;
    Ok(BufWriter::with_capacity(64 * 1024, file))
}
fn open_append_private(path: &Path) -> io::Result<File> {
    reject_symlink(path)?;
    let mut options = OpenOptions::new();
    // Windows append access alone cannot truncate a torn tail with SetEndOfFile.
    // Keep append semantics for frames while also requesting general write access.
    options.create(true).append(true).write(true).read(true);
    configure_private_options(&mut options);
    let file = options.open(path)?;
    validate_private_file(&file)?;
    Ok(file)
}

fn configure_private_options(options: &mut OpenOptions) {
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
}

fn reject_symlink(path: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("refusing symlinked private state file {}", path.display()),
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn validate_private_file(file: &File) -> io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "private state path is not a regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.mode() & 0o077 != 0 || metadata.nlink() != 1 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "private state file must be owner-only and have exactly one link",
            ));
        }
    }
    Ok(())
}
fn codec(error: impl std::fmt::Display) -> io::Error {
    invalid(format!("delta codec: {error}"))
}
fn invalid(error: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.into())
}

#[cfg(test)]
mod tests;
