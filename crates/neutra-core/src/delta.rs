//! Bounded mutable overlay for filesystem change events.
//! The immutable compact base is never rewritten per event; changes are first
//! appended to this owner-only WAL and then reflected in memory.
use crate::FileRecord;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 8] = b"NEUTDLT1";
const HEADER: u64 = 16;
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
        let mut owned_file = if writable {
            Some(open_append_private(path)?)
        } else {
            None
        };
        let file_bytes = if let Some(file) = &owned_file {
            file.metadata()?.len()
        } else {
            std::fs::metadata(path)?.len()
        };
        let mut upserts = HashMap::new();
        let mut removed = HashSet::new();
        let mut wal_bytes = file_bytes;
        if file_bytes > 0 {
            let mut reader = BufReader::new(File::open(path)?.take(file_bytes));
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
            let mut verified_bytes = HEADER;
            loop {
                let mut len = [0u8; 4];
                match reader.read_exact(&mut len) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
                    Err(e) => return Err(e),
                }
                let len = u32::from_le_bytes(len) as usize;
                if len > MAX_FRAME {
                    return Err(invalid("delta frame exceeds safety cap"));
                }
                let mut expected_crc = [0u8; 4];
                match reader.read_exact(&mut expected_crc) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
                    Err(e) => return Err(e),
                }
                let mut payload = vec![0; len];
                match reader.read_exact(&mut payload) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
                    Err(e) => return Err(e),
                }
                let frame_end = verified_bytes + 8 + len as u64;
                if crc32fast::hash(&payload) != u32::from_le_bytes(expected_crc) {
                    if frame_end == file_bytes {
                        break;
                    }
                    return Err(invalid("delta frame checksum mismatch"));
                }
                let change: DeltaChange = bincode::deserialize(&payload).map_err(codec)?;
                apply_memory(&mut upserts, &mut removed, change);
                verified_bytes = frame_end;
            }
            wal_bytes = verified_bytes;
        }
        let writer = if let Some(mut file) = owned_file.take() {
            if file_bytes > wal_bytes {
                file.set_len(wal_bytes)?;
                file.sync_data()?;
            }
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
    pub fn upserts(&self) -> impl Iterator<Item = &FileRecord> {
        self.upserts.values()
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
fn open_append_private(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true).read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
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
        let writer = DeltaIndex::open(&path, 10).unwrap();
        let snapshot = DeltaIndex::open_snapshot(&path, 10).unwrap();
        assert_eq!(snapshot.generation(), 10);
        let error = match DeltaIndex::open(&path, 10) {
            Ok(_) => panic!("second delta writer unexpectedly acquired the lock"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        drop(snapshot);
        drop(writer);
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
        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(&[4, 0])
            .unwrap();
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
