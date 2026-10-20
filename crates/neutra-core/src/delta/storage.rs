use crate::FileRecord;
mod cursor;
mod prefixes;
mod reads;
mod scan;
mod seed;
mod shadow;
mod shared;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
pub(super) use scan::scan_range;
use shadow::ShadowFilter;
use std::io;
use std::path::{Path, PathBuf};

pub(super) use cursor::Cursor;

pub(super) struct DeltaStore {
    db: Option<Connection>,
    stale: bool,
    path: PathBuf,
    private: bool,
    shared: bool,
    shadow: Option<std::sync::Arc<ShadowFilter>>,
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
        Self::validate_root(&overlay_dir)?;
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

    fn validate_root(overlay_dir: &Path) -> io::Result<()> {
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
        Ok(())
    }

    fn open_in(overlay_dir: &Path, wal_path: &Path, private: bool) -> io::Result<Self> {
        Self::validate_root(overlay_dir)?;
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
        if private {
            seed::remove_dead_snapshots(overlay_dir, name);
        }
        super::reject_symlink(&db_path)?;
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true);
        options.create_new(true);
        super::configure_private_options(&mut options);
        match options.open(&db_path) {
            Ok(file) => super::validate_private_file(&file)?,
            Err(error) if !private && error.kind() == io::ErrorKind::AlreadyExists => {
                super::validate_private_metadata(&std::fs::symlink_metadata(&db_path)?)?;
            }
            Err(error) => return Err(error),
        }
        if private {
            seed::seed_snapshot(
                &overlay_dir.join(format!("{}.resolved.sqlite", name.to_string_lossy())),
                &db_path,
            );
        }
        let db = crate::sqlite::open(
            &db_path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(sql_error)?;
        // A lock held by a reader or the sweep is a pause, not corruption:
        // wait for it instead of failing the writer and marking the index stale.
        db.busy_timeout(std::time::Duration::from_secs(60))
            .map_err(sql_error)?;
        // Private snapshots are throwaway copies replayed from the WAL, so
        // they keep the rollback journal in memory, skip fsync and keep a larger page
        // cache: replaying millions of random-order paths is otherwise
        // dominated by journal writes and cache misses.
        let tuning = if private {
            "PRAGMA journal_mode=MEMORY; PRAGMA synchronous=OFF; PRAGMA cache_size=-65536;"
        } else {
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA cache_size=-2048;"
        };
        db.execute_batch(tuning).map_err(sql_error)?;
        db.execute_batch("PRAGMA mmap_size=0; PRAGMA temp_store=FILE; CREATE TABLE IF NOT EXISTS changes(path TEXT PRIMARY KEY NOT NULL, removed INTEGER NOT NULL, record BLOB) WITHOUT ROWID; CREATE TABLE IF NOT EXISTS prefixes(path TEXT PRIMARY KEY NOT NULL) WITHOUT ROWID; CREATE TABLE IF NOT EXISTS meta(k TEXT PRIMARY KEY NOT NULL, v TEXT NOT NULL) WITHOUT ROWID;")
            .map_err(sql_error)?;
        let version: Option<String> = db
            .query_row("SELECT v FROM meta WHERE k='format'", [], |row| row.get(0))
            .optional()
            .map_err(sql_error)?;
        if version.as_deref() != Some("3") {
            // Earlier caches could advance the checkpoint cursor after clearing
            // their rows. Replay the authoritative WAL instead of trusting them.
            db.execute_batch("BEGIN; DELETE FROM changes; DELETE FROM prefixes; DELETE FROM meta; INSERT INTO meta VALUES('format','3'); COMMIT;")
                .map_err(sql_error)?;
        }
        Ok(Self {
            db: Some(db),
            stale: false,
            path: db_path,
            private,
            shared: false,
            shadow: None,
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

    pub(super) fn ensure_live(&self) -> io::Result<()> {
        self.db().map(|_| ())
    }

    pub(super) fn mark_stale(&mut self) {
        self.stale = true;
    }

    fn apply_one(tx: &rusqlite::Transaction, change: &super::DeltaChange) -> io::Result<()> {
        match change {
            super::DeltaChange::Upsert(record) => {
                let bytes = bincode::serialize(record).map_err(sql_error)?;
                tx.execute("INSERT INTO changes(path,removed,record) VALUES(?1,0,?2) ON CONFLICT(path) DO UPDATE SET removed=0,record=excluded.record", params![&*record.path, bytes]).map_err(sql_error)?;
            }
            super::DeltaChange::RemovePrefix(path) => prefixes::apply(tx, path)?,
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
        self.ensure_live()?;
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
        for change in changes {
            match (change, self.shadow.as_mut()) {
                (super::DeltaChange::RemovePrefix(_), _) => self.shadow = None,
                (super::DeltaChange::Upsert(record), Some(filter)) => {
                    std::sync::Arc::make_mut(filter).insert(&record.path)
                }
                (super::DeltaChange::Remove(path), Some(filter)) => {
                    std::sync::Arc::make_mut(filter).insert(path)
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub(super) fn clear(&mut self, cursor: &Cursor) -> io::Result<()> {
        let db = self
            .db
            .as_mut()
            .ok_or_else(|| io::Error::other("delta overlay is closed"))?;
        let tx = db.transaction().map_err(sql_error)?;
        tx.execute("DELETE FROM changes", []).map_err(sql_error)?;
        tx.execute("DELETE FROM prefixes", []).map_err(sql_error)?;
        tx.execute("DELETE FROM meta WHERE k='checkpoint_bytes'", [])
            .map_err(sql_error)?;
        Self::store_cursor_tx(&tx, cursor)?;
        tx.commit().map_err(sql_error)?;
        self.stale = false;
        self.shadow = None;
        Ok(())
    }

    pub(super) fn clear_unknown(&mut self) -> io::Result<()> {
        let db = self
            .db
            .as_mut()
            .ok_or_else(|| io::Error::other("delta overlay is closed"))?;
        let tx = db.transaction().map_err(sql_error)?;
        tx.execute("DELETE FROM changes", []).map_err(sql_error)?;
        tx.execute("DELETE FROM prefixes", []).map_err(sql_error)?;
        tx.execute("DELETE FROM meta WHERE k!='format'", [])
            .map_err(sql_error)?;
        tx.commit().map_err(sql_error)?;
        self.stale = false;
        self.shadow = None;
        Ok(())
    }
}

fn sql_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}
