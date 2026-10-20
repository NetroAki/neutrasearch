use super::{maintenance, signature, storage};
use crate::browse::sql_error;
use crate::{FileRecord, Query};
use rusqlite::{params, Connection};
use std::io;

const RETAINED_EDITS: i64 = 8192;

pub(super) fn append(
    db: &Connection,
    old: Option<&FileRecord>,
    new: Option<&FileRecord>,
    invalidated: bool,
) -> io::Result<()> {
    let old = old
        .map(bincode::serialize)
        .transpose()
        .map_err(io::Error::other)?;
    let new = new
        .map(bincode::serialize)
        .transpose()
        .map_err(io::Error::other)?;
    db.prepare_cached("INSERT INTO search_edits VALUES(NULL,?,?,?,?)")
        .map_err(sql_error)?
        .execute(params![storage::revision(db)? + 1, old, new, invalidated])
        .map_err(sql_error)?;
    Ok(())
}

pub(super) fn trim(db: &Connection) -> io::Result<()> {
    let cutoff: i64 = db.query_row("SELECT coalesce((SELECT sequence FROM search_edits ORDER BY sequence DESC LIMIT 1 OFFSET ?),0)", [RETAINED_EDITS-1], |row| row.get(0)).map_err(sql_error)?;
    db.execute("UPDATE search_floor SET revision=max(revision,coalesce((SELECT max(revision) FROM search_edits WHERE sequence<?),0)) WHERE id=1", [cutoff]).map_err(sql_error)?;
    db.execute("DELETE FROM search_edits WHERE sequence<?", [cutoff])
        .map_err(sql_error)?;
    Ok(())
}

pub(super) fn can_replay(db: &Connection, token: i64) -> io::Result<bool> {
    db.query_row("SELECT ? >= (SELECT revision FROM search_floor WHERE id=1) AND NOT EXISTS(SELECT 1 FROM search_edits WHERE revision>? AND invalidated=1)", params![token,token], |row| row.get(0)).map_err(sql_error)
}

pub(super) fn replay(db: &Connection, token: i64, query: &Query) -> io::Result<()> {
    let mut statement = db
        .prepare(
            "SELECT old_record,new_record FROM search_edits WHERE revision>? ORDER BY sequence",
        )
        .map_err(sql_error)?;
    let mut rows = statement.query([token]).map_err(sql_error)?;
    let matcher = query.matcher()?;
    let spec = signature(query)?;
    while let Some(row) = rows.next().map_err(sql_error)? {
        let old = decode(row.get(0).map_err(sql_error)?)?;
        let new = decode(row.get(1).map_err(sql_error)?)?;
        if let Some(record) = new.as_ref().or(old.as_ref()) {
            maintenance::update_page(
                db,
                &spec,
                &matcher,
                &record.path,
                old.as_ref(),
                new.as_ref(),
            )?;
        }
    }
    Ok(())
}

fn decode(bytes: Option<Vec<u8>>) -> io::Result<Option<FileRecord>> {
    bytes
        .map(|bytes| bincode::deserialize(&bytes).map_err(io::Error::other))
        .transpose()
}
