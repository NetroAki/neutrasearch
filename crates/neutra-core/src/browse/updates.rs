use super::{sql_error, BrowserIndex, SCHEMA_VERSION};
use crate::{DeltaChange, DeltaIndex};
use rusqlite::{params, Connection, OpenFlags};
use std::io;
use std::path::{Path, PathBuf};

pub(super) fn base_path(index: &Path) -> PathBuf {
    let mut path = BrowserIndex::path_for(index).into_os_string();
    path.push(".base");
    path.into()
}

pub(super) fn pin_base(index: &Path) -> io::Result<()> {
    let destination = base_path(index);
    let temporary = destination.with_extension("base.new");
    if temporary.exists() {
        std::fs::remove_file(&temporary)?;
    }
    std::fs::hard_link(index, &temporary)?;
    crate::compact_build::replace_file(&temporary, &destination)?;
    crate::compact_build::sync_parent(&destination)
}

pub(super) fn upgrade(index: &Path, generation: u64) -> io::Result<bool> {
    let path = BrowserIndex::path_for(index);
    if !path.is_file() {
        return Ok(false);
    }
    let mut db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(sql_error)?;
    let (found, version): (String, i64) = db
        .query_row("SELECT generation,version FROM metadata", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .map_err(sql_error)?;
    if found != generation.to_string() {
        return Ok(false);
    }
    if version == SCHEMA_VERSION {
        return Ok(BrowserIndex::is_current(index, generation));
    }
    if version != 1 {
        return Ok(false);
    }
    pin_base(index)?;
    let transaction = db.transaction().map_err(sql_error)?;
    transaction.execute_batch("ALTER TABLE entries ADD COLUMN record BLOB; ALTER TABLE metadata ADD COLUMN base_generation TEXT;").map_err(sql_error)?;
    transaction.execute_batch("CREATE INDEX IF NOT EXISTS by_ext ON entries(ext,id); CREATE INDEX IF NOT EXISTS by_programs ON entries(executable,kind,id);").map_err(sql_error)?;
    transaction
        .execute(
            "UPDATE metadata SET version=?,base_generation=generation",
            [SCHEMA_VERSION],
        )
        .map_err(sql_error)?;
    transaction.commit().map_err(sql_error)?;
    Ok(true)
}

impl BrowserIndex {
    /// Commit metadata changes into the disk sort orders. The immutable pinned
    /// record blocks remain valid even when the active compact base is replaced.
    pub fn apply_changes(index: &Path, generation: u64, changes: &[DeltaChange]) -> io::Result<()> {
        let path = Self::path_for(index);
        if !path.is_file() {
            return Ok(());
        }
        let mut db = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(sql_error)?;
        db.busy_timeout(std::time::Duration::from_secs(2))
            .map_err(sql_error)?;
        db.execute_batch("PRAGMA cache_size=-4096; PRAGMA mmap_size=0; PRAGMA journal_mode=WAL;")
            .map_err(sql_error)?;
        let found: (String, i64) = db
            .query_row("SELECT generation,version FROM metadata", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .map_err(sql_error)?;
        if found != (generation.to_string(), SCHEMA_VERSION) {
            return Ok(());
        }
        let transaction = db.transaction().map_err(sql_error)?;
        for change in changes {
            let path = match change {
                DeltaChange::Upsert(record) => &record.path,
                DeltaChange::Remove(path) => path,
            };
            if let DeltaChange::Upsert(record) = change {
                let bytes = bincode::serialize(record).map_err(io::Error::other)?;
                use rusqlite::OptionalExtension;
                let stored: Option<Vec<u8>> = transaction
                    .prepare_cached(
                        "SELECT record FROM entries WHERE path=? AND record IS NOT NULL LIMIT 1",
                    )
                    .map_err(sql_error)?
                    .query_row([&**path], |row| row.get(0))
                    .optional()
                    .map_err(sql_error)?;
                if stored.as_ref() == Some(&bytes) {
                    continue;
                }
            }
            transaction
                .execute("DELETE FROM entries WHERE path=?", [&**path])
                .map_err(sql_error)?;
            if let DeltaChange::Upsert(record) = change {
                let mtime = if record.mtime > 4_102_444_800 {
                    0
                } else {
                    record.mtime
                };
                let bytes = bincode::serialize(record).map_err(io::Error::other)?;
                transaction
                    .execute(
                        "INSERT INTO entries VALUES(NULL,?,?,?,?,?,?,?,?,?)",
                        params![
                            &*record.path,
                            record.name(),
                            &record.disk_bytes().to_be_bytes()[..],
                            mtime,
                            record.kind as i64,
                            record.extension().to_ascii_lowercase(),
                            record.fs.label(),
                            i64::from(crate::query::is_executable(record)),
                            bytes
                        ],
                    )
                    .map_err(sql_error)?;
            }
        }
        transaction.commit().map_err(sql_error)
    }

    pub fn apply_delta(index: &Path, delta: &DeltaIndex) -> io::Result<()> {
        let mut batch = Vec::new();
        let mut apply = |change| -> io::Result<()> {
            batch.push(change);
            if batch.len() == 4096 {
                Self::apply_changes(index, delta.generation(), &batch)?;
                batch.clear();
            }
            Ok(())
        };
        delta.for_each_removed(|path| apply(DeltaChange::Remove(path)))?;
        delta.for_each_upsert(|record| apply(DeltaChange::Upsert(record)))?;
        if !batch.is_empty() {
            Self::apply_changes(index, delta.generation(), &batch)?;
        }
        Ok(())
    }

    /// Compaction changes block addresses; the catalog already holds its live
    /// changes and keeps using the pinned original blocks for unchanged rows.
    pub fn advance_generation(index: &Path, old: u64, new: u64) -> io::Result<()> {
        if !Self::path_for(index).is_file() {
            return Ok(());
        }
        let db = Connection::open_with_flags(
            Self::path_for(index),
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(sql_error)?;
        db.busy_timeout(std::time::Duration::from_secs(2))
            .map_err(sql_error)?;
        db.execute(
            "UPDATE metadata SET generation=? WHERE generation=? AND version=?",
            params![new.to_string(), old.to_string(), SCHEMA_VERSION],
        )
        .map_err(sql_error)?;
        Ok(())
    }

    /// Complete a marked publication whose base is already visible. The
    /// compactor commits catalog mutations before creating that marker.
    pub fn recover_generation(index: &Path, generation: u64) -> io::Result<()> {
        if !Self::path_for(index).is_file() {
            return Ok(());
        }
        let db = Connection::open_with_flags(
            Self::path_for(index),
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(sql_error)?;
        db.busy_timeout(std::time::Duration::from_secs(2))
            .map_err(sql_error)?;
        db.execute(
            "UPDATE metadata SET generation=? WHERE version=?",
            params![generation.to_string(), SCHEMA_VERSION],
        )
        .map_err(sql_error)?;
        Ok(())
    }
}
