use super::*;
use crate::record_ref::RecordRef;

impl DeltaStore {
    pub(in crate::delta) fn open_shared(wal: &Path) -> io::Result<Self> {
        let root = Self::ensure_root(wal)?;
        let name = wal
            .file_name()
            .ok_or_else(|| io::Error::other("missing WAL name"))?;
        let mut db_name = name.to_os_string();
        db_name.push(".resolved.sqlite");
        let path = root.join(db_name);
        super::super::reject_symlink(&path)?;
        // Closing an unrelated descriptor can release this process's SQLite
        // POSIX locks. Inspect the path without opening the live database.
        super::super::validate_private_metadata(&std::fs::symlink_metadata(&path)?)?;
        let db = crate::sqlite::open(
            &path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(sql_error)?;
        db.busy_timeout(std::time::Duration::from_millis(100))
            .map_err(sql_error)?;
        db.execute_batch("PRAGMA cache_size=-4096; PRAGMA mmap_size=0; BEGIN;")
            .map_err(sql_error)?;
        let mode: String = db
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .map_err(sql_error)?;
        let format: String = db
            .query_row("SELECT v FROM meta WHERE k='format'", [], |row| row.get(0))
            .map_err(sql_error)?;
        if mode != "wal" || format != "3" {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "live cache needs WAL snapshot support",
            ));
        }
        Ok(Self {
            db: Some(db),
            stale: false,
            path,
            private: false,
            shared: true,
            shadow: None,
        })
    }

    pub(in crate::delta) fn is_shared(&self) -> bool {
        self.shared
    }

    pub(in crate::delta) fn scan_shared(
        &self,
        mut visit: impl FnMut(&RecordRef<'_>) -> io::Result<()>,
    ) -> io::Result<()> {
        let mut statement = self
            .db()?
            .prepare("SELECT record FROM changes WHERE removed=0")
            .map_err(sql_error)?;
        let mut rows = statement.query([]).map_err(sql_error)?;
        while let Some(row) = rows.next().map_err(sql_error)? {
            let blob = row
                .get_ref(0)
                .map_err(sql_error)?
                .as_blob()
                .map_err(sql_error)?;
            let view: RecordRef<'_> = bincode::deserialize(blob).map_err(sql_error)?;
            visit(&view)?;
        }
        Ok(())
    }
}
