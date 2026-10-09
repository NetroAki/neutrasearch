use crate::FileRecord;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use std::io;
use std::path::{Path, PathBuf};

/// Verified WAL position the overlay embodies. Committed in the same SQLite
/// transaction as the frames it describes, so a crash leaves either the old
/// cursor (the tail replays on next open) or the new cursor with the
/// complete resolved state. The WAL stays authoritative in all cases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Cursor {
    pub generation: u64,
    pub wal_bytes: u64,
    pub legacy_frames: bool,
    /// Modification time of the WAL at the verified cursor, in nanoseconds
    /// since the Unix epoch. Detects same-length in-place edits.
    pub modified_ns: u128,
    #[cfg(unix)]
    pub dev: u64,
    #[cfg(unix)]
    pub ino: u64,
}

pub(super) struct DeltaStore {
    db: Option<Connection>,
    stale: bool,
    path: PathBuf,
    private: bool,
}

impl Drop for DeltaStore {
    fn drop(&mut self) {
        if let Some(db) = self.db.take() {
            let _ = db.close();
        }
        if self.private {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

impl DeltaStore {
    pub(super) fn open(wal_path: &Path) -> io::Result<Self> {
        let overlay_dir = Self::ensure_root(wal_path)?;
        Self::open_in(&overlay_dir, wal_path, false)
    }

    /// Private point-in-time overlay for read-only snapshots. Same
    /// disk-backed excluded root (never tmpfs), unique file, removed on
    /// drop. Snapshots never mutate the writer's shared cache, so a
    /// concurrent checkpoint/reset cannot interleave rows or cursors.
    pub(super) fn open_private(wal_path: &Path) -> io::Result<Self> {
        let overlay_dir = Self::ensure_root(wal_path)?;
        Self::open_in(&overlay_dir, wal_path, true)
    }

    fn ensure_root(wal_path: &Path) -> io::Result<PathBuf> {
        let root = wal_path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let overlay_dir = root.join(".delta.overlay");
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            match std::fs::DirBuilder::new().mode(0o700).create(&overlay_dir) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
        }
        #[cfg(not(unix))]
        match std::fs::create_dir(&overlay_dir) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        Ok(overlay_dir)
    }

    /// Watcher exclusion roots for this WAL: the overlay directory covers
    /// the shared cache, its `-journal`, and any private snapshot files.
    /// Watchers must ignore these paths; they are not source events.
    pub(crate) fn exclusion_roots(wal_path: &Path) -> Vec<PathBuf> {
        let root = wal_path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        vec![root.join(".delta.overlay")]
    }

    fn open_in(overlay_dir: &Path, wal_path: &Path, private: bool) -> io::Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let m = std::fs::symlink_metadata(overlay_dir)?;
            if !m.is_dir()
                || m.file_type().is_symlink()
                || m.mode() & 0o077 != 0
                || m.uid() != unsafe { libc::geteuid() }
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "delta overlay root must be owner-only and owned by this user",
                ));
            }
        }
        let name = wal_path.file_name().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "delta WAL needs a filename")
        })?;
        let mut db_name = name.to_os_string();
        if private {
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            db_name.push(format!(".snapshot-{}-{id}.sqlite", std::process::id()));
        } else {
            db_name.push(".resolved.sqlite");
        }
        let db_path = overlay_dir.join(db_name);
        super::reject_symlink(&db_path)?;
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true);
        if private {
            options.create_new(true);
        } else {
            options.create(true).truncate(false);
        }
        super::configure_private_options(&mut options);
        let file = options.open(&db_path)?;
        super::validate_private_file(&file)?;
        drop(file);
        let db = Connection::open_with_flags(
            &db_path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(sql_error)?;
        db.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=NORMAL; PRAGMA cache_size=-2048; PRAGMA mmap_size=0; PRAGMA temp_store=FILE; CREATE TABLE IF NOT EXISTS changes(path TEXT PRIMARY KEY NOT NULL, removed INTEGER NOT NULL, record BLOB) WITHOUT ROWID; CREATE TABLE IF NOT EXISTS meta(k TEXT PRIMARY KEY NOT NULL, v TEXT NOT NULL) WITHOUT ROWID;")
            .map_err(sql_error)?;
        let version: Option<String> = db
            .query_row("SELECT v FROM meta WHERE k='format'", [], |row| row.get(0))
            .optional()
            .map_err(sql_error)?;
        if version.as_deref() != Some("2") {
            // Earlier caches could advance the checkpoint cursor after clearing
            // their rows. Replay the authoritative WAL instead of trusting them.
            db.execute_batch("BEGIN; DELETE FROM changes; DELETE FROM meta; INSERT INTO meta VALUES('format','2'); COMMIT;")
                .map_err(sql_error)?;
        }
        Ok(Self {
            db: Some(db),
            stale: false,
            path: db_path,
            private,
        })
    }

    fn db(&self) -> io::Result<&Connection> {
        if self.stale {
            return Err(io::Error::other("delta overlay is stale; reopen the index"));
        }
        self.db
            .as_ref()
            .ok_or_else(|| io::Error::other("delta overlay is closed"))
    }

    pub(super) fn mark_stale(&mut self) {
        self.stale = true;
    }

    pub(super) fn cursor(&self) -> io::Result<Option<Cursor>> {
        let db = self.db()?;
        let get = |key: &str| -> io::Result<Option<String>> {
            db.query_row("SELECT v FROM meta WHERE k=?1", [key], |row| row.get(0))
                .optional()
                .map_err(sql_error)
        };
        let (Some(generation), Some(wal_bytes), Some(legacy)) =
            (get("generation")?, get("wal_bytes")?, get("legacy_frames")?)
        else {
            return Ok(None);
        };
        let Some(modified_ns) = get("modified_ns")? else {
            return Ok(None);
        };
        #[cfg(unix)]
        let (Some(dev), Some(ino)) = (get("dev")?, get("ino")?) else {
            return Ok(None);
        };
        let parse = |key: &str, v: &str| {
            v.parse::<u64>().map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("delta overlay cursor {key} corrupt"),
                )
            })
        };
        Ok(Some(Cursor {
            generation: parse("generation", &generation)?,
            wal_bytes: parse("wal_bytes", &wal_bytes)?,
            legacy_frames: legacy != "0",
            modified_ns: modified_ns.parse::<u128>().map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "delta overlay cursor modified_ns corrupt",
                )
            })?,
            #[cfg(unix)]
            dev: parse("dev", &dev)?,
            #[cfg(unix)]
            ino: parse("ino", &ino)?,
        }))
    }

    pub(super) fn set_cursor(&mut self, cursor: &Cursor) -> io::Result<()> {
        let db = self
            .db
            .as_mut()
            .ok_or_else(|| io::Error::other("delta overlay is closed"))?;
        let tx = db.transaction().map_err(sql_error)?;
        Self::store_cursor_tx(&tx, cursor)?;
        tx.commit().map_err(sql_error)
    }

    pub(super) fn checkpoint_bytes(&self) -> io::Result<Option<u64>> {
        let found: Option<String> = self
            .db()?
            .query_row("SELECT v FROM meta WHERE k='checkpoint_bytes'", [], |row| {
                row.get(0)
            })
            .optional()
            .map_err(sql_error)?;
        found
            .map(|bytes| bytes.parse().map_err(io::Error::other))
            .transpose()
    }

    pub(super) fn set_checkpoint_cursor(&mut self, cursor: &Cursor) -> io::Result<()> {
        let db = self
            .db
            .as_mut()
            .ok_or_else(|| io::Error::other("delta overlay is closed"))?;
        let tx = db.transaction().map_err(sql_error)?;
        Self::store_cursor_tx(&tx, cursor)?;
        tx.execute(
            "INSERT OR REPLACE INTO meta VALUES('checkpoint_bytes',?1)",
            [cursor.wal_bytes.to_string()],
        )
        .map_err(sql_error)?;
        tx.commit().map_err(sql_error)
    }

    fn store_cursor_tx(tx: &rusqlite::Transaction, cursor: &Cursor) -> io::Result<()> {
        let legacy = if cursor.legacy_frames { "1" } else { "0" };
        tx.execute("INSERT INTO meta(k,v) VALUES('generation',?1) ON CONFLICT(k) DO UPDATE SET v=excluded.v", [cursor.generation.to_string()]).map_err(sql_error)?;
        tx.execute("INSERT INTO meta(k,v) VALUES('wal_bytes',?1) ON CONFLICT(k) DO UPDATE SET v=excluded.v", [cursor.wal_bytes.to_string()]).map_err(sql_error)?;
        tx.execute("INSERT INTO meta(k,v) VALUES('legacy_frames',?1) ON CONFLICT(k) DO UPDATE SET v=excluded.v", [legacy]).map_err(sql_error)?;
        tx.execute("INSERT INTO meta(k,v) VALUES('modified_ns',?1) ON CONFLICT(k) DO UPDATE SET v=excluded.v", [cursor.modified_ns.to_string()]).map_err(sql_error)?;
        #[cfg(unix)]
        {
            tx.execute(
                "INSERT INTO meta(k,v) VALUES('dev',?1) ON CONFLICT(k) DO UPDATE SET v=excluded.v",
                [cursor.dev.to_string()],
            )
            .map_err(sql_error)?;
            tx.execute(
                "INSERT INTO meta(k,v) VALUES('ino',?1) ON CONFLICT(k) DO UPDATE SET v=excluded.v",
                [cursor.ino.to_string()],
            )
            .map_err(sql_error)?;
        }
        Ok(())
    }

    fn apply_one(tx: &rusqlite::Transaction, change: &super::DeltaChange) -> io::Result<()> {
        match change {
            super::DeltaChange::Upsert(record) => {
                let bytes = bincode::serialize(record).map_err(sql_error)?;
                tx.execute("INSERT INTO changes(path,removed,record) VALUES(?1,0,?2) ON CONFLICT(path) DO UPDATE SET removed=0,record=excluded.record", params![&*record.path, bytes]).map_err(sql_error)?;
            }
            super::DeltaChange::Remove(path) => {
                tx.execute("INSERT INTO changes(path,removed,record) VALUES(?1,1,NULL) ON CONFLICT(path) DO UPDATE SET removed=1,record=NULL", [&**path]).map_err(sql_error)?;
            }
        }
        Ok(())
    }

    /// Apply decoded frames and advance the cursor atomically: one
    /// transaction per bounded caller batch keeps `-journal` churn flat and
    /// guarantees the cursor never describes frames the table lacks.
    pub(super) fn apply_batch(
        &mut self,
        changes: &[super::DeltaChange],
        cursor: &Cursor,
    ) -> io::Result<()> {
        let db = self
            .db
            .as_mut()
            .ok_or_else(|| io::Error::other("delta overlay is closed"))?;
        let tx = db.transaction().map_err(sql_error)?;
        for change in changes {
            Self::apply_one(&tx, change)?;
        }
        Self::store_cursor_tx(&tx, cursor)?;
        tx.commit().map_err(sql_error)?;
        Ok(())
    }

    pub(super) fn clear(&mut self, cursor: &Cursor) -> io::Result<()> {
        let db = self
            .db
            .as_mut()
            .ok_or_else(|| io::Error::other("delta overlay is closed"))?;
        let tx = db.transaction().map_err(sql_error)?;
        tx.execute("DELETE FROM changes", []).map_err(sql_error)?;
        tx.execute("DELETE FROM meta WHERE k='checkpoint_bytes'", [])
            .map_err(sql_error)?;
        Self::store_cursor_tx(&tx, cursor)?;
        tx.commit().map_err(sql_error)?;
        self.stale = false;
        Ok(())
    }

    pub(super) fn clear_unknown(&mut self) -> io::Result<()> {
        let db = self
            .db
            .as_mut()
            .ok_or_else(|| io::Error::other("delta overlay is closed"))?;
        let tx = db.transaction().map_err(sql_error)?;
        tx.execute("DELETE FROM changes", []).map_err(sql_error)?;
        tx.execute("DELETE FROM meta WHERE k!='format'", [])
            .map_err(sql_error)?;
        tx.commit().map_err(sql_error)?;
        self.stale = false;
        Ok(())
    }

    pub(super) fn get(&self, path: &str) -> io::Result<Option<FileRecord>> {
        let bytes: Option<Option<Vec<u8>>> = self
            .db()?
            .query_row("SELECT record FROM changes WHERE path=?1", [path], |row| {
                row.get(0)
            })
            .optional()
            .map_err(sql_error)?;
        bytes
            .flatten()
            .map(|bytes| bincode::deserialize(&bytes).map_err(sql_error))
            .transpose()
    }

    pub(super) fn is_removed(&self, path: &str) -> io::Result<bool> {
        self.db()?
            .query_row("SELECT removed FROM changes WHERE path=?1", [path], |row| {
                row.get::<_, bool>(0)
            })
            .optional()
            .map(|v| v.unwrap_or(false))
            .map_err(sql_error)
    }

    pub(super) fn shadows(&self, path: &str) -> io::Result<bool> {
        self.db()?
            .query_row("SELECT 1 FROM changes WHERE path=?1", [path], |_| Ok(()))
            .optional()
            .map(|v| v.is_some())
            .map_err(sql_error)
    }

    pub(super) fn count(&self) -> io::Result<usize> {
        let count: i64 = self
            .db()?
            .query_row("SELECT count(*) FROM changes", [], |r| r.get(0))
            .map_err(sql_error)?;
        usize::try_from(count).map_err(|_| io::Error::other("delta change count overflow"))
    }

    pub(super) fn each(
        &self,
        removed: bool,
        mut f: impl FnMut(&str, Option<FileRecord>) -> io::Result<()>,
    ) -> io::Result<()> {
        let mut statement = self
            .db()?
            .prepare(if removed {
                "SELECT path,NULL FROM changes WHERE removed=1 ORDER BY path"
            } else {
                "SELECT path,record FROM changes WHERE removed=0 ORDER BY path"
            })
            .map_err(sql_error)?;
        let mut rows = statement.query([]).map_err(sql_error)?;
        while let Some(row) = rows.next().map_err(sql_error)? {
            let path: String = row.get(0).map_err(sql_error)?;
            let bytes: Option<Vec<u8>> = row.get(1).map_err(sql_error)?;
            let record = bytes
                .map(|b| bincode::deserialize(&b).map_err(sql_error))
                .transpose()?;
            f(&path, record)?;
        }
        Ok(())
    }

    pub(super) fn shadows_many(&self, paths: &[&str]) -> io::Result<Vec<bool>> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let mut out = Vec::with_capacity(paths.len());
        for chunk in paths.chunks(400) {
            let marks = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("SELECT path FROM changes WHERE path IN ({marks})");
            let mut statement = self.db()?.prepare(&sql).map_err(sql_error)?;
            let found = statement
                .query_map(rusqlite::params_from_iter(chunk.iter()), |r| {
                    r.get::<_, String>(0)
                })
                .map_err(sql_error)?
                .collect::<Result<std::collections::HashSet<_>, _>>()
                .map_err(sql_error)?;
            out.extend(chunk.iter().map(|p| found.contains(*p)));
        }
        Ok(out)
    }
}

fn sql_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}
