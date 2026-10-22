//! Catalog identity: which base rows are still live and where a path lives.
//!
//! Base rows are addressed by id in the pinned base. A change that replaces or
//! removes one records its id here; changed or new rows live in the SQLite
//! "entries" table, which stays small because it holds only live differences.

use super::columns::BlockReader;
use super::{sql_error, BrowserIndex};
use crate::FileRecord;
use rusqlite::{params, Connection, OptionalExtension};
use std::{io, sync::Arc};

/// Replaced or removed base ids, as single ids and half-open id intervals.
#[derive(Default)]
pub(super) struct Tombstones {
    singles: Vec<u32>,
    ranges: Vec<(u32, u32)>,
}

impl Tombstones {
    pub(super) fn load(db: &Connection) -> io::Result<Self> {
        let mut singles = Vec::new();
        let mut statement = db
            .prepare("SELECT id FROM tomb ORDER BY id")
            .map_err(sql_error)?;
        let mut rows = statement.query([]).map_err(sql_error)?;
        while let Some(row) = rows.next().map_err(sql_error)? {
            singles.push(row.get::<_, i64>(0).map_err(sql_error)? as u32);
        }
        let mut ranges: Vec<(u32, u32)> = Vec::new();
        let mut statement = db
            .prepare("SELECT low,high FROM tomb_range ORDER BY low")
            .map_err(sql_error)?;
        let mut rows = statement.query([]).map_err(sql_error)?;
        while let Some(row) = rows.next().map_err(sql_error)? {
            let (low, high) = (
                row.get::<_, i64>(0).map_err(sql_error)? as u32,
                row.get::<_, i64>(1).map_err(sql_error)? as u32,
            );
            match ranges.last_mut() {
                Some(last) if low <= last.1 => last.1 = last.1.max(high),
                _ => ranges.push((low, high)),
            }
        }
        Ok(Self { singles, ranges })
    }

    pub(super) fn contains(&self, id: u32) -> bool {
        if self.singles.binary_search(&id).is_ok() {
            return true;
        }
        let at = self.ranges.partition_point(|&(low, _)| low <= id);
        at > 0 && id < self.ranges[at - 1].1
    }

    /// First id at or after the argument that is not tombstoned.
    pub(super) fn next_live(&self, mut id: u32) -> u32 {
        loop {
            let at = self.ranges.partition_point(|&(low, _)| low <= id);
            if at > 0 && id < self.ranges[at - 1].1 {
                id = self.ranges[at - 1].1;
            } else if self.singles.binary_search(&id).is_ok() {
                id += 1;
            } else {
                return id;
            }
        }
    }
}

/// Single-id probe for writers that hold a transaction but no loaded set.
pub(super) fn is_dead(db: &Connection, id: u32) -> io::Result<bool> {
    db.prepare_cached(
        "SELECT EXISTS(SELECT 1 FROM tomb WHERE id=?1) OR EXISTS(SELECT 1 FROM tomb_range WHERE low<=?1 AND high>?1)",
    )
    .map_err(sql_error)?
    .query_row([i64::from(id)], |row| row.get(0))
    .map_err(sql_error)
}

pub(super) fn bump_revision(db: &Connection) -> io::Result<()> {
    db.execute(
        "UPDATE tomb_revision SET revision=revision+1 WHERE id=1",
        [],
    )
    .map_err(sql_error)?;
    Ok(())
}

/// Ids of base rows whose path equals the argument (one per source).
pub(super) fn base_ids_for_path(reader: &mut BlockReader<'_>, path: &str) -> io::Result<Vec<u32>> {
    let total = reader.base().len() as u32;
    let mut id = reader.path_lower_bound(path)?;
    let mut found = Vec::new();
    while id < total {
        let record = reader.record(id)?;
        if crate::compact::compare_index_paths(&record.path, path) != std::cmp::Ordering::Equal {
            break;
        }
        if record.path.as_ref() == path {
            found.push(id);
        }
        id += 1;
    }
    Ok(found)
}

/// Half-open id intervals holding the path itself and everything beneath it.
pub(super) fn base_subtree(
    reader: &mut BlockReader<'_>,
    path: &str,
) -> io::Result<[(u32, u32); 2]> {
    let root = path.trim_end_matches('/');
    let exact = reader.path_lower_bound(root)?;
    let after = reader.path_lower_bound(&format!("{root}\0"))?;
    let low = reader.path_lower_bound(&format!("{root}/"))?;
    let high = reader.path_lower_bound(&format!("{root}0"))?;
    Ok([(exact, after), (low, high)])
}

/// A live row for the path, or the live base row when none replaced it.
pub(super) fn find_record(
    db: &Connection,
    reader: &mut BlockReader<'_>,
    path: &str,
) -> io::Result<Option<FileRecord>> {
    let live: Option<Vec<u8>> = db
        .prepare_cached("SELECT record FROM entries WHERE path=? AND record IS NOT NULL LIMIT 1")
        .map_err(sql_error)?
        .query_row([path], |row| row.get(0))
        .optional()
        .map_err(sql_error)?;
    if let Some(bytes) = live {
        return bincode::deserialize(&bytes)
            .map(Some)
            .map_err(io::Error::other);
    }
    for id in base_ids_for_path(reader, path)? {
        if !is_dead(db, id)? {
            return Ok(Some(reader.record(id)?.clone()));
        }
    }
    Ok(None)
}

impl BrowserIndex {
    pub(super) fn read_snapshot(&self) -> io::Result<rusqlite::Transaction<'_>> {
        let snapshot = self.db.unchecked_transaction().map_err(sql_error)?;
        self.require_pinned_base()?;
        Ok(snapshot)
    }

    // A replaced SQL catalog must not be joined to this reader's old columns.
    pub(super) fn require_pinned_base(&self) -> io::Result<()> {
        let found: (String, i64) = self
            .db
            .query_row("SELECT base_generation,version FROM metadata", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .map_err(sql_error)?;
        if found != (self.base.generation().to_string(), super::SCHEMA_VERSION) {
            return Err(io::Error::other(
                "browse catalog was replaced; reopen the reader",
            ));
        }
        Ok(())
    }

    /// Cached tombstone set; reloaded when a writer bumps the revision.
    pub(super) fn tombstones(&self) -> io::Result<Arc<Tombstones>> {
        let revision: i64 = self
            .db
            .query_row("SELECT revision FROM tomb_revision WHERE id=1", [], |row| {
                row.get(0)
            })
            .map_err(sql_error)?;
        let mut cache = self
            .tombstone_cache
            .lock()
            .map_err(|_| io::Error::other("tombstone cache poisoned"))?;
        if let Some((held, set)) = cache.as_ref() {
            if *held == revision {
                return Ok(Arc::clone(set));
            }
        }
        let set = Arc::new(Tombstones::load(&self.db)?);
        *cache = Some((revision, Arc::clone(&set)));
        Ok(set)
    }

    /// Resolve a watcher lookup: the live row if one exists, else the base row.
    pub fn record_by_path(&self, path: &str) -> io::Result<Option<FileRecord>> {
        let _snapshot = self.read_snapshot()?;
        find_record(&self.db, &mut BlockReader::new(&self.base), path)
    }

    pub(super) fn tombstone_insert(db: &Connection, id: u32) -> io::Result<bool> {
        let changed = db
            .prepare_cached("INSERT OR IGNORE INTO tomb VALUES(?)")
            .map_err(sql_error)?
            .execute(params![i64::from(id)])
            .map_err(sql_error)?;
        Ok(changed > 0)
    }
}
