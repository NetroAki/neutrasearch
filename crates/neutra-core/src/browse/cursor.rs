use super::{sql_error, BrowserIndex, SCHEMA_VERSION};
use crate::DeltaIndex;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use std::{io, path::Path};

fn stamp(path: &Path) -> io::Result<(String, String, String)> {
    let metadata = std::fs::metadata(path)?;
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        format!("{}:{}", metadata.dev(), metadata.ino())
    };
    #[cfg(not(unix))]
    let identity = String::new();
    let modified = metadata
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos()
        .to_string();
    Ok((identity, metadata.len().to_string(), modified))
}

impl BrowserIndex {
    /// Publish only after every durable WAL frame is reflected in the catalog.
    pub fn mark_delta(index: &Path, delta: &DeltaIndex) -> io::Result<()> {
        let path = Self::path_for(index);
        if !path.is_file() {
            return Ok(());
        }
        let (identity, bytes, modified) = stamp(delta.path())?;
        if bytes != delta.wal_bytes().to_string() {
            return Err(io::Error::other(
                "delta changed before catalog cursor publication",
            ));
        }
        let mut db = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(sql_error)?;
        db.busy_timeout(std::time::Duration::from_secs(2))
            .map_err(sql_error)?;
        db.execute_batch("PRAGMA cache_size=-1024; PRAGMA mmap_size=0;")
            .map_err(sql_error)?;
        let found: (String, i64) = db
            .query_row("SELECT generation,version FROM metadata", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .map_err(sql_error)?;
        if found != (delta.generation().to_string(), SCHEMA_VERSION) {
            return Ok(());
        }
        let transaction = db.transaction().map_err(sql_error)?;
        transaction.execute_batch("CREATE TABLE IF NOT EXISTS live_cursor(id INTEGER PRIMARY KEY CHECK(id=1),generation TEXT NOT NULL,identity TEXT NOT NULL,bytes TEXT NOT NULL,modified TEXT NOT NULL);").map_err(sql_error)?;
        transaction
            .execute(
                "INSERT OR REPLACE INTO live_cursor VALUES(1,?1,?2,?3,?4)",
                params![delta.generation().to_string(), identity, bytes, modified],
            )
            .map_err(sql_error)?;
        transaction.commit().map_err(sql_error)
    }

    /// A matching cursor lets ordinary listings skip replaying the live WAL.
    pub fn covers_delta(&self, index: &Path) -> io::Result<bool> {
        let path = index.with_extension("delta");
        let stamp = match stamp(&path) {
            Ok(stamp) => stamp,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(true),
            Err(error) => return Err(error),
        };
        let exists = self
            .db
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='live_cursor'",
                [],
                |_| Ok(()),
            )
            .optional()
            .map_err(sql_error)?
            .is_some();
        if !exists {
            return Ok(false);
        }
        let found: Option<(String, String, String, String)> = self
            .db
            .query_row(
                "SELECT generation,identity,bytes,modified FROM live_cursor WHERE id=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(sql_error)?;
        Ok(found == Some((self.generation.to_string(), stamp.0, stamp.1, stamp.2)))
    }
}
