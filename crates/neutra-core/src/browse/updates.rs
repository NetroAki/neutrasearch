use super::columns::BlockReader;
use super::{catalog, sql_error, BrowserIndex, SCHEMA_VERSION};
use crate::{DeltaChange, DeltaIndex};
use rusqlite::{params, OpenFlags};
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

fn writer(index: &Path) -> io::Result<rusqlite::Connection> {
    let db = crate::sqlite::open(
        BrowserIndex::path_for(index),
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(sql_error)?;
    db.busy_timeout(std::time::Duration::from_secs(60))
        .map_err(sql_error)?;
    Ok(db)
}

pub(in crate::browse) mod writer_cache;

impl BrowserIndex {
    /// Commit metadata changes into the catalog. Replaced or removed base rows
    /// become tombstones; new and changed rows go into the small live table.
    /// The pinned base blocks stay valid when the active compact base changes.
    pub fn apply_changes(index: &Path, generation: u64, changes: &[DeltaChange]) -> io::Result<()> {
        if !Self::path_for(index).is_file() {
            return Ok(());
        }
        let mut db = writer(index)?;
        db.execute_batch("PRAGMA cache_size=-4096; PRAGMA mmap_size=0; PRAGMA journal_mode=WAL;")
            .map_err(sql_error)?;
        // Own the write reservation before reading metadata or cache state.
        // Otherwise a cache writer can commit between our read and write,
        // invalidating the deferred transaction's snapshot.
        let transaction = db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        let found: (String, i64, String) = transaction
            .query_row(
                "SELECT generation,version,base_generation FROM metadata",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(sql_error)?;
        if found.0 != generation.to_string() || found.1 != SCHEMA_VERSION {
            return Ok(());
        }
        let held = writer_cache::open(index, &found.2)?;
        let mut reader = BlockReader::with_cache(&held.base, &held.decoded);
        let mut pages = super::search_cache::Maintenance::open(&transaction)?;
        let mut tombstoned = false;
        for change in changes {
            tombstoned |= apply_one(&transaction, &mut reader, &mut pages, change)?;
        }
        pages.finish(&transaction)?;
        if tombstoned {
            catalog::bump_revision(&transaction)?;
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
        delta.for_each_removed_prefix(|path| apply(DeltaChange::RemovePrefix(path)))?;
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
        writer(index)?
            .execute(
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
        writer(index)?
            .execute(
                "UPDATE metadata SET generation=? WHERE version=?",
                params![generation.to_string(), SCHEMA_VERSION],
            )
            .map_err(sql_error)?;
        Ok(())
    }
}

/// Apply one change; returns whether a base row was newly tombstoned.
fn apply_one(
    tx: &rusqlite::Transaction<'_>,
    reader: &mut BlockReader<'_>,
    pages: &mut super::search_cache::Maintenance,
    change: &DeltaChange,
) -> io::Result<bool> {
    let mut tombstoned = false;
    if let DeltaChange::RemovePrefix(path) = change {
        pages.remove_prefix(tx, reader, path)?;
        for (low, high) in catalog::base_subtree(reader, path)? {
            if low < high {
                tx.execute(
                    "INSERT INTO tomb_range VALUES(?1,?2) ON CONFLICT(low) DO UPDATE SET high=max(high,excluded.high)",
                    params![i64::from(low), i64::from(high)],
                )
                .map_err(sql_error)?;
                tombstoned = true;
            }
        }
        let prefix = format!("{}/", path.trim_end_matches('/'));
        let mut past = prefix.clone();
        past.pop();
        past.push('0');
        tx.execute(
            "DELETE FROM entries WHERE path=?1 OR (path>=?2 AND path<?3)",
            params![&**path, prefix, past],
        )
        .map_err(sql_error)?;
        return Ok(tombstoned);
    }
    let path = match change {
        DeltaChange::Upsert(record) => &record.path,
        DeltaChange::Remove(path) | DeltaChange::RemovePrefix(path) => path,
    };
    let old = catalog::find_record(tx, reader, path)?;
    let bytes = match change {
        DeltaChange::Upsert(record) => Some(bincode::serialize(record).map_err(io::Error::other)?),
        _ => None,
    };
    if let (Some(bytes), Some(old)) = (&bytes, &old) {
        if bincode::serialize(old).map_err(io::Error::other)? == *bytes {
            return Ok(false);
        }
    }
    pages.change(tx, change, old.as_ref())?;
    for id in catalog::base_ids_for_path(reader, path)? {
        tombstoned |= BrowserIndex::tombstone_insert(tx, id)?;
    }
    tx.execute("DELETE FROM entries WHERE path=?", [&**path])
        .map_err(sql_error)?;
    if let (DeltaChange::Upsert(record), Some(bytes)) = (change, bytes) {
        let mtime = if record.mtime > 4_102_444_800 {
            0
        } else {
            record.mtime
        };
        tx.execute(
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
    Ok(tombstoned)
}
