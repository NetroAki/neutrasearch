use super::{tally::Counts, SLOTS, VERSION};
use crate::browse::catalog;
use crate::browse::columns::BlockReader;
use crate::FileRecord;
use rusqlite::{params, Connection, OptionalExtension};
use std::io;

pub(in crate::browse) fn ready(db: &Connection) -> io::Result<Option<bool>> {
    let table_count: i64 = db
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ('short_ready','short_counts')",
            [],
            |r| r.get(0),
        ).map_err(super::super::sql_error)?;
    if table_count != 2 {
        return Ok(None);
    }
    let row: Option<(i64, i64)> = db
        .query_row(
            "SELECT version,unix_paths FROM short_ready LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(super::super::sql_error)?;
    match row {
        Some((VERSION, unix_paths)) => Ok(Some(unix_paths != 0)),
        _ => Ok(None),
    }
}

pub(in crate::browse) fn matched(db: &Connection, code: usize) -> io::Result<u64> {
    let count: i64 = db
        .query_row(
            "SELECT matched FROM short_counts WHERE code=?",
            [code as i64],
            |r| r.get(0),
        )
        .map_err(super::super::sql_error)?;
    u64::try_from(count)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "negative short count"))
}

pub(in crate::browse) fn update(
    db: &Connection,
    old: Option<&FileRecord>,
    new: Option<&FileRecord>,
) -> io::Result<()> {
    if ready(db)?.is_none() {
        return Ok(());
    }
    if old.map(FileRecord::name) == new.map(FileRecord::name) {
        if new.is_some_and(|record| !record.path.starts_with('/')) {
            db.execute("UPDATE short_ready SET unix_paths=0", [])
                .map_err(super::super::sql_error)?;
        }
        return Ok(());
    }
    let mut delta = Counts::default();
    if let Some(record) = old {
        delta.add_name(record.name(), -1);
    }
    if let Some(record) = new {
        delta.add_name(record.name(), 1);
    }
    apply(db, &delta.slots)?;
    if new.is_some_and(|record| !record.path.starts_with('/')) {
        db.execute("UPDATE short_ready SET unix_paths=0", [])
            .map_err(super::super::sql_error)?;
    }
    Ok(())
}

/// Subtract the names of every live row under a path. Call before the
/// subtree's base rows are tombstoned and its live rows deleted.
pub(in crate::browse) fn remove_prefix(
    db: &Connection,
    reader: &mut BlockReader<'_>,
    path: &str,
) -> io::Result<()> {
    if ready(db)?.is_none() {
        return Ok(());
    }
    let root = path.trim_end_matches('/');
    let prefix = format!("{root}/");
    let mut past = prefix.clone();
    past.pop();
    past.push('0');
    let mut delta = Counts::default();
    {
        let mut statement = db
            .prepare_cached("SELECT name FROM entries INDEXED BY by_path WHERE path=?1 OR (path>=?2 AND path<?3)")
            .map_err(super::super::sql_error)?;
        let mut rows = statement
            .query(params![path, prefix, past])
            .map_err(super::super::sql_error)?;
        while let Some(row) = rows.next().map_err(super::super::sql_error)? {
            let name: String = row.get(0).map_err(super::super::sql_error)?;
            delta.add_name(&name, -1);
        }
    }
    for (low, high) in catalog::base_subtree(reader, path)? {
        for id in low..high {
            if !catalog::is_dead(db, id)? {
                delta.add_name(reader.record(id)?.name(), -1);
            }
        }
    }
    apply(db, &delta.slots)
}

fn apply(db: &Connection, delta: &[i64; SLOTS]) -> io::Result<()> {
    let mut statement = db
        .prepare_cached("UPDATE short_counts SET matched=matched+?1 WHERE code=?2")
        .map_err(super::super::sql_error)?;
    for (code, change) in delta.iter().enumerate().filter(|(_, change)| **change != 0) {
        let changed = statement
            .execute(params![*change, code as i64])
            .map_err(super::super::sql_error)?;
        if changed != 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "incomplete short counts table",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn update_and_prefix_removal_keep_exact_counts() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE entries(path TEXT PRIMARY KEY,name TEXT NOT NULL); CREATE INDEX by_path ON entries(path);")
            .unwrap();
        let mut initial = Counts::default();
        initial.add_name("Alpha", 1);
        initial.add_name("Beta", 1);
        initial.add_name("Beta", 1);
        initial.add_name("Beta", 1);
        initial.publish(&db, true).unwrap();
        db.execute("INSERT INTO entries VALUES('/a/Alpha','Alpha')", [])
            .unwrap();
        db.execute("INSERT INTO entries VALUES('/a/Beta','Beta')", [])
            .unwrap();
        db.execute("INSERT INTO entries VALUES('/root','Beta')", [])
            .unwrap();
        db.execute("INSERT INTO entries VALUES('/root/kid','Beta')", [])
            .unwrap();
        db.execute("INSERT INTO entries VALUES('/rooted','Beta')", [])
            .unwrap();
        let (old, new) = (record("/a/Alpha"), record("/a/Gamma"));
        update(&db, Some(&old), Some(&new)).unwrap();
        db.execute("DELETE FROM entries WHERE path=?", [&*old.path])
            .unwrap();
        db.execute("INSERT INTO entries VALUES(?,'Gamma')", [&*new.path])
            .unwrap();
        assert_eq!(
            matched(&db, super::super::tally::key("al").unwrap()).unwrap(),
            0
        );
        update(&db, Some(&new), Some(&new)).unwrap();
        assert_eq!(
            matched(&db, super::super::tally::key("ga").unwrap()).unwrap(),
            1
        );
        db.execute(
            "DELETE FROM entries WHERE path='/root' OR (path>='/root/' AND path<'/root0')",
            [],
        )
        .unwrap();
        update(&db, Some(&record("/root/Beta")), None).unwrap();
        update(&db, Some(&record("/root/kid/Beta")), None).unwrap();
        assert_eq!(
            matched(&db, super::super::tally::key("be").unwrap()).unwrap(),
            1
        );
        let remaining: i64 = db
            .query_row("SELECT count(*) FROM entries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(remaining, 3);
    }

    fn record(path: &str) -> FileRecord {
        FileRecord {
            path: path.into(),
            size: 0,
            disk: 0,
            mtime: 0,
            mode: 0,
            kind: crate::FileKind::File,
            fs: crate::mounts::FsKind::Ext4,
            native_id: 0,
            native_parent: 0,
            source: 0,
        }
    }
}
