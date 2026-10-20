use super::{ordering, MAX_PAGES};
use crate::browse::{sql_error, BrowserIndex};
use crate::{Query, SearchHit};
use rusqlite::{params, Connection, OpenFlags};
use std::{io, path::Path};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS search_revision(id INTEGER PRIMARY KEY CHECK(id=1),revision INTEGER NOT NULL);
INSERT OR IGNORE INTO search_revision VALUES(1,0);
CREATE TABLE IF NOT EXISTS search_floor(id INTEGER PRIMARY KEY CHECK(id=1),revision INTEGER NOT NULL);
INSERT OR IGNORE INTO search_floor VALUES(1,0);
CREATE TABLE IF NOT EXISTS search_edits(sequence INTEGER PRIMARY KEY,revision INTEGER NOT NULL,
 old_record BLOB,new_record BLOB,invalidated INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS search_edits_revision ON search_edits(revision);
CREATE TABLE IF NOT EXISTS search_pages(spec BLOB PRIMARY KEY,matched INTEGER NOT NULL,
 capacity INTEGER NOT NULL,boundary BLOB);
CREATE TABLE IF NOT EXISTS search_rows(spec BLOB NOT NULL,path TEXT NOT NULL,rank BLOB NOT NULL,
 score INTEGER NOT NULL,record BLOB NOT NULL,PRIMARY KEY(spec,rank)) WITHOUT ROWID;
CREATE UNIQUE INDEX IF NOT EXISTS search_rows_path ON search_rows(spec,path);
";

pub(super) fn exists(db: &Connection) -> io::Result<bool> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='search_revision' AND type='table')",
        [],
        |row| row.get(0),
    )
    .map_err(sql_error)
}

pub(super) fn writer(index: &Path) -> io::Result<Connection> {
    let db = crate::sqlite::open(
        BrowserIndex::path_for(index),
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(sql_error)?;
    db.busy_timeout(std::time::Duration::from_secs(2))
        .map_err(sql_error)?;
    db.execute_batch("PRAGMA cache_size=-2048; PRAGMA mmap_size=0; PRAGMA journal_mode=WAL;")
        .map_err(sql_error)?;
    Ok(db)
}

pub(super) fn ensure(index: &Path) -> io::Result<()> {
    let mut db = writer(index)?;
    let tx = db
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(sql_error)?;
    tx.execute_batch(SCHEMA).map_err(sql_error)?;
    tx.commit().map_err(sql_error)
}

pub(super) fn revision(db: &Connection) -> io::Result<i64> {
    db.query_row(
        "SELECT revision FROM search_revision WHERE id=1",
        [],
        |row| row.get(0),
    )
    .map_err(sql_error)
}

pub(super) fn insert(
    db: &Connection,
    spec: &[u8],
    query: &Query,
    hit: &SearchHit,
) -> io::Result<()> {
    let rank = ordering::key(query.sort, hit.score, &hit.record);
    let record = bincode::serialize(&hit.record).map_err(io::Error::other)?;
    db.prepare_cached("INSERT OR REPLACE INTO search_rows VALUES(?,?,?,?,?)")
        .map_err(sql_error)?
        .execute(params![spec, &*hit.record.path, rank, hit.score, record])
        .map_err(sql_error)?;
    Ok(())
}

pub(super) fn clear(db: &Connection) -> io::Result<()> {
    db.execute_batch("DELETE FROM search_rows; DELETE FROM search_pages;")
        .map_err(sql_error)
}

pub(super) fn evict(db: &Connection) -> io::Result<()> {
    db.execute("DELETE FROM search_pages WHERE rowid NOT IN (SELECT rowid FROM search_pages ORDER BY rowid DESC LIMIT ?)", [MAX_PAGES as i64]).map_err(sql_error)?;
    db.execute(
        "DELETE FROM search_rows WHERE spec NOT IN (SELECT spec FROM search_pages)",
        [],
    )
    .map_err(sql_error)?;
    Ok(())
}
