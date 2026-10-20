use super::{journal, ordering, storage};
use crate::browse::columns::BlockReader;
use crate::browse::sql_error;
use crate::{DeltaChange, FileRecord, Query, QueryMatcher, SearchHit};
use rusqlite::{params, Connection};
use std::io;

struct Page {
    spec: Vec<u8>,
    matcher: QueryMatcher,
}

pub(in crate::browse) struct Maintenance {
    enabled: bool,
    short: bool,
    pages: Vec<Page>,
}

impl Maintenance {
    pub(in crate::browse) fn open(db: &Connection) -> io::Result<Self> {
        let enabled = storage::exists(db)?;
        let short = crate::browse::short_counts::ready(db)?.is_some();
        let mut pages = Vec::new();
        if enabled {
            let mut statement = db
                .prepare("SELECT spec FROM search_pages")
                .map_err(sql_error)?;
            let specs = statement
                .query_map([], |row| row.get::<_, Vec<u8>>(0))
                .map_err(sql_error)?;
            for spec in specs {
                let spec = spec.map_err(sql_error)?;
                let query: Query = match bincode::deserialize(&spec) {
                    Ok(query) => query,
                    Err(_) => {
                        storage::clear(db)?;
                        pages.clear();
                        break;
                    }
                };
                pages.push(Page {
                    spec,
                    matcher: query.matcher()?,
                });
            }
        }
        Ok(Self {
            enabled,
            short,
            pages,
        })
    }

    /// Drop the page cache and correct short counts before a subtree goes.
    pub(in crate::browse) fn remove_prefix(
        &mut self,
        db: &Connection,
        reader: &mut BlockReader<'_>,
        path: &str,
    ) -> io::Result<()> {
        if !self.enabled && !self.short {
            return Ok(());
        }
        crate::browse::short_counts::remove_prefix(db, reader, path)?;
        // A subtree tombstone must stay constant-size, including cache invalidation.
        if self.enabled {
            storage::clear(db)?;
            self.pages.clear();
            journal::append(db, None, None, true)?;
        }
        Ok(())
    }

    /// `old` is the record this change replaces or removes, if any.
    pub(in crate::browse) fn change(
        &mut self,
        db: &Connection,
        change: &DeltaChange,
        old: Option<&FileRecord>,
    ) -> io::Result<()> {
        if !self.enabled && !self.short {
            return Ok(());
        }
        let (path, new) = match change {
            DeltaChange::Upsert(record) => (&*record.path, Some(record)),
            DeltaChange::Remove(path) => (&**path, None),
            DeltaChange::RemovePrefix(_) => return Ok(()),
        };
        crate::browse::short_counts::update(db, old, new)?;
        if self.enabled {
            journal::append(db, old, new, false)?;
        }
        for page in &self.pages {
            update_page(db, &page.spec, &page.matcher, path, old, new)?;
        }
        Ok(())
    }

    pub(in crate::browse) fn finish(&self, db: &Connection) -> io::Result<()> {
        if self.enabled {
            db.execute(
                "UPDATE search_revision SET revision=revision+1 WHERE id=1",
                [],
            )
            .map_err(sql_error)?;
            journal::trim(db)?;
        }
        Ok(())
    }
}

fn score(matcher: &QueryMatcher, record: &FileRecord) -> Option<u32> {
    matcher
        .query()
        .passes_filters(record)
        .then(|| matcher.score(record))
        .flatten()
}

pub(super) fn update_page(
    db: &Connection,
    spec: &[u8],
    matcher: &QueryMatcher,
    path: &str,
    old: Option<&FileRecord>,
    new: Option<&FileRecord>,
) -> io::Result<()> {
    let (mut matched, capacity, boundary): (i64, i64, Option<Vec<u8>>) = db
        .query_row(
            "SELECT matched,capacity,boundary FROM search_pages WHERE spec=?",
            [spec],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(sql_error)?;
    if old.is_some_and(|record| score(matcher, record).is_some()) {
        matched -= 1;
    }
    db.prepare_cached("DELETE FROM search_rows WHERE spec=? AND path=?")
        .map_err(sql_error)?
        .execute(params![spec, path])
        .map_err(sql_error)?;
    if let Some((record, score)) =
        new.and_then(|record| score(matcher, record).map(|score| (record, score)))
    {
        matched += 1;
        let rank = ordering::key(matcher.query().sort, score, record);
        if boundary.as_ref().is_none_or(|boundary| rank <= *boundary) {
            storage::insert(
                db,
                spec,
                matcher.query(),
                &SearchHit {
                    score,
                    record: record.clone(),
                },
            )?;
            trim(db, spec, capacity)?;
        }
    }
    if matched < 0 {
        return Err(io::Error::other("negative search page count"));
    }
    db.prepare_cached("UPDATE search_pages SET matched=? WHERE spec=?")
        .map_err(sql_error)?
        .execute(params![matched, spec])
        .map_err(sql_error)?;
    Ok(())
}

fn trim(db: &Connection, spec: &[u8], capacity: i64) -> io::Result<()> {
    let count: i64 = db
        .query_row(
            "SELECT count(*) FROM search_rows WHERE spec=?",
            [spec],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    if count <= capacity {
        return Ok(());
    }
    let boundary: Vec<u8> = db
        .query_row(
            "SELECT rank FROM search_rows WHERE spec=? ORDER BY rank LIMIT 1 OFFSET ?",
            params![spec, capacity - 1],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    db.execute(
        "DELETE FROM search_rows WHERE spec=? AND rank>?",
        params![spec, boundary],
    )
    .map_err(sql_error)?;
    db.execute(
        "UPDATE search_pages SET boundary=? WHERE spec=?",
        params![boundary, spec],
    )
    .map_err(sql_error)?;
    Ok(())
}
