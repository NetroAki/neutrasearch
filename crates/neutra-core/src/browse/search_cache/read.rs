use super::{ordering, signature, storage, CAPACITY};
use crate::browse::{sql_error, BrowserIndex};
use crate::{CompactIndex, FileRecord, Query, SearchHit, SearchStats};
use rusqlite::{params, OptionalExtension};
use std::{io, path::Path, time::Instant};

impl BrowserIndex {
    pub fn has_prepared_search(&self, query: &Query) -> io::Result<bool> {
        if !storage::exists(&self.db)? {
            return Ok(false);
        }
        self.db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM search_pages WHERE spec=?)",
                [signature(query)?],
                |row| row.get(0),
            )
            .map_err(sql_error)
    }

    /// None means this page is unprepared, exhausted, or behind the live WAL.
    pub fn search_cached(
        &self,
        index: &Path,
        query: &Query,
        after: Option<&FileRecord>,
    ) -> io::Result<Option<(Vec<SearchHit>, SearchStats)>> {
        if CompactIndex::generation_on_disk(index)? != self.generation {
            return Ok(None);
        }
        if query.limit == 0 || query.limit > CAPACITY || !storage::exists(&self.db)? {
            return Ok(None);
        }
        let started = Instant::now();
        let snapshot = self.read_snapshot()?;
        if !self.covers_delta(index)? {
            return Ok(None);
        }
        let spec = signature(query)?;
        let page: Option<(i64, Option<Vec<u8>>)> = snapshot
            .query_row(
                "SELECT matched,boundary FROM search_pages WHERE spec=?",
                [&spec],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(sql_error)?;
        let Some((matched, boundary)) = page else {
            return Ok(None);
        };
        let matched = u64::try_from(matched).map_err(io::Error::other)?;
        let anchor = after
            .map(|record| {
                let score = query.matcher()?.score(record).unwrap_or(0);
                Ok::<_, io::Error>(ordering::key(query.sort, score, record))
            })
            .transpose()?;
        let mut statement = snapshot.prepare("SELECT score,record FROM search_rows WHERE spec=?1 AND (?2 IS NULL OR rank>?2) ORDER BY rank LIMIT ?3").map_err(sql_error)?;
        let mut rows = statement
            .query(params![spec, anchor, query.limit as i64])
            .map_err(sql_error)?;
        let mut hits = Vec::with_capacity(query.limit);
        while let Some(row) = rows.next().map_err(sql_error)? {
            let bytes: Vec<u8> = row.get(1).map_err(sql_error)?;
            hits.push(SearchHit {
                score: row.get(0).map_err(sql_error)?,
                record: bincode::deserialize(&bytes).map_err(io::Error::other)?,
            });
        }
        if boundary.is_some() && hits.len() < query.limit {
            return Ok(None);
        }
        Ok(Some((
            hits,
            SearchStats {
                scanned: 0,
                matched,
                wall_us: started.elapsed().as_micros() as u64,
            },
        )))
    }
}
