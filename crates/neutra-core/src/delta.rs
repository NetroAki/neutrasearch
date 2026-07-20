//! Bounded mutable overlay for filesystem change events.
//! The immutable compact base is never rewritten per event; changes are first
//! appended to this owner-only WAL and then reflected in memory.
use crate::FileRecord;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 8] = b"NEUTDLT1";
pub const DELTA_HEADER_BYTES: u64 = 16;
const HEADER: u64 = DELTA_HEADER_BYTES;
const MAX_FRAME: usize = 16 * 1024 * 1024;
pub const DEFAULT_COMPACT_AT: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DeltaChange {
    Upsert(FileRecord),
    Remove(Box<str>),
}

pub struct DeltaIndex {
    path: PathBuf,
    generation: u64,
    writer: Option<BufWriter<File>>,
    _lock_file: Option<File>,
    upserts: HashMap<Box<str>, FileRecord>,
    removed: HashSet<Box<str>>,
    wal_bytes: u64,
    compact_at: u64,
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
        Ok(Self {
            path: path.to_path_buf(),
            generation,
            writer: Some(writer),
            _lock_file: Some(lock_file),
            upserts: HashMap::new(),
            removed: HashSet::new(),
            wal_bytes: HEADER,
            compact_at: compact_at.max(HEADER + 1),
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
        let file_bytes = match std::fs::metadata(path) {
            Ok(metadata) => metadata.len(),
            Err(error) if writable && error.kind() == io::ErrorKind::NotFound => 0,
            Err(error) => return Err(error),
        };
        let mut upserts = HashMap::new();
        let mut removed = HashSet::new();
        let mut wal_bytes = file_bytes;
        if file_bytes > 0 {
            let mut reader = BufReader::new(File::open(path)?.take(file_bytes));
            read_header(&mut reader, generation)?;
            let mut verified_bytes = HEADER;
            replay_frames(&mut reader, &mut verified_bytes, &mut upserts, &mut removed)?;
            wal_bytes = verified_bytes;
        }
        let writer = if writable {
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
        } else {
            None
        };
        Ok(Self {
            path: path.to_path_buf(),
            generation,
            writer,
            _lock_file: lock_file,
            upserts,
            removed,
            wal_bytes,
            compact_at: compact_at.max(HEADER + 1),
        })
    }
    pub fn apply(&mut self, change: DeltaChange) -> io::Result<()> {
        let payload = bincode::serialize(&change).map_err(codec)?;
        if payload.len() > MAX_FRAME {
            return Err(invalid("delta change exceeds safety cap"));
        }
        let writer = self.writer.as_mut().ok_or_else(|| {
            io::Error::new(io::ErrorKind::PermissionDenied, "read-only delta snapshot")
        })?;
        writer.write_all(&(payload.len() as u32).to_le_bytes())?;
        writer.write_all(&crc32fast::hash(&payload).to_le_bytes())?;
        writer.write_all(&payload)?;
        writer.flush()?;
        self.wal_bytes = self.wal_bytes.saturating_add(payload.len() as u64 + 8);
        apply_memory(&mut self.upserts, &mut self.removed, change);
        Ok(())
    }
    pub fn sync(&mut self) -> io::Result<()> {
        let writer = self.writer.as_mut().ok_or_else(|| {
            io::Error::new(io::ErrorKind::PermissionDenied, "read-only delta snapshot")
        })?;
        writer.flush()?;
        writer.get_ref().sync_data()
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
        self.writer = Some(writer);
        self.generation = generation;
        self.upserts.clear();
        self.removed.clear();
        self.wal_bytes = HEADER;
        Ok(())
    }
    /// Tail complete CRC-verified frames appended since this read-only snapshot
    /// was opened. A concurrently written partial final frame remains invisible
    /// until a later refresh.
    pub fn refresh(&mut self) -> io::Result<u64> {
        if self.writer.is_some() {
            return Ok(0);
        }
        let file_bytes = std::fs::metadata(&self.path)?.len();
        let mut file = File::open(&self.path)?;
        // A compaction reset can keep an empty WAL at the same byte length.
        // Validate the generation before the length fast path so persistent
        // readers never continue serving the old mmap base silently.
        read_header(&mut file, self.generation)?;
        if file_bytes < self.wal_bytes {
            return Err(invalid("delta log was replaced or truncated"));
        }
        if file_bytes == self.wal_bytes {
            return Ok(0);
        }
        file.seek(SeekFrom::Start(self.wal_bytes))?;
        let mut reader = BufReader::new(file.take(file_bytes - self.wal_bytes));
        let old_bytes = self.wal_bytes;
        replay_frames(
            &mut reader,
            &mut self.wal_bytes,
            &mut self.upserts,
            &mut self.removed,
        )?;
        Ok(self.wal_bytes - old_bytes)
    }
    pub fn upserts(&self) -> impl Iterator<Item = &FileRecord> {
        self.upserts.values()
    }
    pub fn removed(&self) -> impl Iterator<Item = &Box<str>> {
        self.removed.iter()
    }
    pub fn is_removed(&self, path: &str) -> bool {
        self.removed.contains(path)
    }
    pub fn shadows(&self, path: &str) -> bool {
        self.removed.contains(path) || self.upserts.contains_key(path)
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn wal_bytes(&self) -> u64 {
        self.wal_bytes
    }
    pub fn change_count(&self) -> usize {
        self.upserts.len() + self.removed.len()
    }
    pub fn needs_compaction(&self) -> bool {
        self.wal_bytes >= self.compact_at
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
}
fn read_header(reader: &mut impl Read, generation: u64) -> io::Result<()> {
    let mut magic = [0u8; 8];
    reader.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(invalid("not a Neutrasearch delta log"));
    }
    let mut stored_generation = [0u8; 8];
    reader.read_exact(&mut stored_generation)?;
    if u64::from_le_bytes(stored_generation) != generation {
        return Err(invalid("delta log belongs to a different base generation"));
    }
    Ok(())
}

fn replay_frames(
    reader: &mut impl Read,
    verified_bytes: &mut u64,
    upserts: &mut HashMap<Box<str>, FileRecord>,
    removed: &mut HashSet<Box<str>>,
) -> io::Result<()> {
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
        let change: DeltaChange = bincode::deserialize(&payload).map_err(codec)?;
        apply_memory(upserts, removed, change);
        *verified_bytes = frame_end;
    }
    Ok(())
}

fn apply_memory(
    upserts: &mut HashMap<Box<str>, FileRecord>,
    removed: &mut HashSet<Box<str>>,
    change: DeltaChange,
) {
    match change {
        DeltaChange::Upsert(record) => {
            removed.remove(record.path.as_ref());
            upserts.insert(record.path.clone(), record);
        }
        DeltaChange::Remove(path) => {
            upserts.remove(path.as_ref());
            removed.insert(path);
        }
    }
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
mod tests {
    use super::*;
    use crate::{FileKind, FsKind};
    fn record(path: &str, size: u64) -> FileRecord {
        FileRecord {
            path: path.into(),
            size,
            mtime: 0,
            mode: 0,
            kind: FileKind::File,
            fs: FsKind::Btrfs,
            native_id: 0,
            native_parent: 0,
            source: 0,
        }
    }
    fn remove_log(path: &Path) {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(lock_path(path));
    }

    #[test]
    fn permits_snapshots_but_rejects_a_second_writer() {
        let path =
            std::env::temp_dir().join(format!("neutra-delta-lock-{}.wal", std::process::id()));
        remove_log(&path);
        let mut writer = DeltaIndex::open(&path, 10).unwrap();
        writer
            .apply(DeltaChange::Upsert(record("/first", 1)))
            .unwrap();
        writer.sync().unwrap();
        let mut snapshot = DeltaIndex::open_snapshot(&path, 10).unwrap();
        assert_eq!(snapshot.generation(), 10);
        assert_eq!(snapshot.upserts().count(), 1);
        let error = match DeltaIndex::open(&path, 10) {
            Ok(_) => panic!("second delta writer unexpectedly acquired the lock"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        writer
            .apply(DeltaChange::Upsert(record("/second", 2)))
            .unwrap();
        writer.sync().unwrap();
        assert!(snapshot.refresh().unwrap() > 0);
        assert_eq!(snapshot.upserts().count(), 2);
        drop(snapshot);
        drop(writer);
        remove_log(&path);
    }

    #[test]
    fn complete_frame_with_bad_checksum_fails_closed() {
        let path =
            std::env::temp_dir().join(format!("neutra-delta-corrupt-{}.wal", std::process::id()));
        remove_log(&path);
        let mut delta = DeltaIndex::open(&path, 11).unwrap();
        delta.apply(DeltaChange::Upsert(record("/a", 1))).unwrap();
        delta.sync().unwrap();
        drop(delta);

        let mut bytes = std::fs::read(&path).unwrap();
        *bytes.last_mut().unwrap() ^= 0x80;
        std::fs::write(&path, bytes).unwrap();
        assert!(DeltaIndex::open(&path, 11).is_err());
        remove_log(&path);
    }

    #[test]
    fn torn_tail_is_truncated_before_new_appends() {
        let path =
            std::env::temp_dir().join(format!("neutra-delta-tail-{}.wal", std::process::id()));
        remove_log(&path);
        {
            let mut delta = DeltaIndex::open(&path, 9).unwrap();
            delta.apply(DeltaChange::Upsert(record("/a", 1))).unwrap();
            delta.sync().unwrap();
        }
        let verified_bytes = std::fs::metadata(&path).unwrap().len();
        let mut torn = OpenOptions::new().write(true).open(&path).unwrap();
        torn.seek(SeekFrom::End(0)).unwrap();
        torn.write_all(&[4, 0]).unwrap();
        drop(torn);
        {
            let mut recovered = DeltaIndex::open(&path, 9).unwrap();
            assert_eq!(recovered.wal_bytes(), verified_bytes);
            assert_eq!(std::fs::metadata(&path).unwrap().len(), verified_bytes);
            recovered
                .apply(DeltaChange::Upsert(record("/b", 2)))
                .unwrap();
            recovered.sync().unwrap();
        }
        let reopened = DeltaIndex::open(&path, 9).unwrap();
        assert_eq!(reopened.upserts().count(), 2);
        drop(reopened);
        remove_log(&path);
    }

    #[cfg(unix)]
    #[test]
    fn writer_refuses_symlinked_wal_and_lock_files() {
        use std::os::unix::fs::symlink;

        let path =
            std::env::temp_dir().join(format!("neutra-delta-symlink-{}.wal", std::process::id()));
        let victim = path.with_extension("victim");
        remove_log(&path);
        let _ = std::fs::remove_file(&victim);
        std::fs::write(&victim, b"do not truncate").unwrap();
        symlink(&victim, &path).unwrap();
        assert!(DeltaIndex::replace_empty(&path, 7).is_err());
        assert_eq!(std::fs::read(&victim).unwrap(), b"do not truncate");
        std::fs::remove_file(&path).unwrap();
        remove_log(&path);

        let lock = lock_path(&path);
        symlink(&victim, &lock).unwrap();
        assert!(DeltaIndex::open(&path, 7).is_err());
        assert_eq!(std::fs::read(&victim).unwrap(), b"do not truncate");
        std::fs::remove_file(lock).unwrap();
        let _ = std::fs::remove_file(path);
        std::fs::remove_file(victim).unwrap();
    }

    #[test]
    fn reset_rebinds_the_empty_wal_without_releasing_the_writer_lock() {
        let path =
            std::env::temp_dir().join(format!("neutra-delta-reset-{}.wal", std::process::id()));
        remove_log(&path);
        let mut delta = DeltaIndex::open(&path, 7).unwrap();
        let mut stale_snapshot = DeltaIndex::open_snapshot(&path, 7).unwrap();
        delta.apply(DeltaChange::Upsert(record("/a", 1))).unwrap();
        delta.sync().unwrap();

        delta.reset(8).unwrap();
        assert!(stale_snapshot.refresh().is_err());
        drop(stale_snapshot);
        assert_eq!(delta.generation(), 8);
        assert_eq!(delta.wal_bytes(), HEADER);
        assert_eq!(delta.change_count(), 0);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), HEADER);
        assert!(matches!(
            DeltaIndex::open(&path, 8),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock
        ));
        drop(delta);

        let reopened = DeltaIndex::open(&path, 8).unwrap();
        assert_eq!(reopened.change_count(), 0);
        drop(reopened);
        assert!(DeltaIndex::open(&path, 7).is_err());
        remove_log(&path);
    }

    #[test]
    fn wal_replays_upserts_and_tombstones() {
        let path = std::env::temp_dir().join(format!("neutra-delta-{}.wal", std::process::id()));
        remove_log(&path);
        {
            let mut delta = DeltaIndex::open_with_threshold(&path, 7, 1).unwrap();
            delta.apply(DeltaChange::Upsert(record("/a", 1))).unwrap();
            delta.apply(DeltaChange::Upsert(record("/b", 2))).unwrap();
            delta.apply(DeltaChange::Remove("/a".into())).unwrap();
            delta.sync().unwrap();
            assert!(delta.needs_compaction());
        }
        let delta = DeltaIndex::open(&path, 7).unwrap();
        assert!(delta.is_removed("/a"));
        assert_eq!(delta.upserts().next().unwrap().path.as_ref(), "/b");
        drop(delta);
        assert!(DeltaIndex::open(&path, 8).is_err());
        remove_log(&path);
    }
}
