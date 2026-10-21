use super::{journal, ordering, signature, storage, CAPACITY};
use crate::browse::{sql_error, BrowserIndex, SCHEMA_VERSION};
use crate::{CompactIndex, DeltaIndex, FileRecord, Query, SearchHit, SearchStats};
use rusqlite::params;
use std::{io, path::Path};

#[cfg(test)]
mod tests;

impl BrowserIndex {
    /// Prepare spare rows once; normal file events maintain counts and ordering.
    /// Deep pages fall back when they pass the retained, proven result frontier.
    pub fn search_prepared(
        &self,
        index: &Path,
        base: &CompactIndex,
        query: &Query,
        delta: Option<&DeltaIndex>,
        after: Option<&FileRecord>,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> io::Result<(Vec<SearchHit>, SearchStats)> {
        // A newly published index carries an empty WAL even for base-only callers.
        let empty_delta = if delta.is_none()
            && std::fs::metadata(index.with_extension("delta"))
                .is_ok_and(|m| m.len() == crate::DELTA_HEADER_BYTES)
        {
            Some(DeltaIndex::open_snapshot(
                &index.with_extension("delta"),
                base.generation(),
            )?)
        } else {
            None
        };
        let delta = delta.or(empty_delta.as_ref());
        if cancelled() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "search superseded",
            ));
        }
        if CompactIndex::generation_on_disk(index)? != self.generation {
            return Err(io::Error::other("prepared search index was replaced"));
        }
        match self.search_cached(index, query, after) {
            Ok(Some(found)) => return Ok(found),
            Ok(None) => {}
            Err(error) => eprintln!("Prepared search unavailable: {error}"),
        }
        if let Some(found) = self.search_short(index, query, after, cancelled)? {
            return Ok(found);
        }
        if base.generation() != self.generation {
            return Err(io::Error::other("browse/base generation mismatch"));
        }
        let can_prepare = after.is_none()
            && query.limit > 0
            && query.limit <= CAPACITY
            && (!query.terms.is_empty() || query.regex.is_some());
        let token = if can_prepare {
            self.preparation_token(index, delta)
                .unwrap_or_else(|error| {
                    eprintln!("Cannot prepare search: {error}");
                    None
                })
        } else {
            None
        };
        let mut expanded = query.clone();
        if token.is_some() {
            expanded.limit = CAPACITY;
        }
        let (mut hits, stats) =
            base.search_after_interruptible(&expanded, delta, after, cancelled)?;
        if let Some(token) = token {
            if !cancelled() {
                if let Err(error) = self.save_page(index, query, token, &hits, stats.matched) {
                    eprintln!("Cannot retain prepared search: {error}");
                }
            }
        }
        hits.truncate(query.limit);
        Ok((hits, stats))
    }

    fn preparation_token(
        &self,
        index: &Path,
        delta: Option<&DeltaIndex>,
    ) -> io::Result<Option<i64>> {
        storage::ensure(index)?;
        let snapshot = self.db.unchecked_transaction().map_err(sql_error)?;
        let token = storage::revision(&snapshot)?;
        if !self.covers_delta(index)? {
            return Ok(None);
        }
        let wal_len = match std::fs::metadata(index.with_extension("delta")) {
            Ok(meta) => Some(meta.len()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        if wal_len != delta.map(DeltaIndex::wal_bytes) {
            return Ok(None);
        }
        Ok(Some(token))
    }

    fn save_page(
        &self,
        index: &Path,
        query: &Query,
        token: i64,
        hits: &[SearchHit],
        matched: u64,
    ) -> io::Result<()> {
        let mut db = storage::writer(index)?;
        let tx = db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        let metadata: (String, i64) = tx
            .query_row("SELECT generation,version FROM metadata", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .map_err(sql_error)?;
        if metadata != (self.generation.to_string(), SCHEMA_VERSION)
            || !journal::can_replay(&tx, token)?
        {
            return Ok(());
        }
        let spec = signature(query)?;
        tx.execute("DELETE FROM search_rows WHERE spec=?", [&spec])
            .map_err(sql_error)?;
        let boundary = if matched > hits.len() as u64 {
            hits.last()
                .map(|hit| ordering::key(query.sort, hit.score, &hit.record))
        } else {
            None
        };
        let matched = i64::try_from(matched).map_err(io::Error::other)?;
        tx.execute(
            "INSERT OR REPLACE INTO search_pages VALUES(?,?,?,?)",
            params![spec, matched, CAPACITY as i64, boundary],
        )
        .map_err(sql_error)?;
        for hit in hits {
            storage::insert(&tx, &spec, query, hit)?;
        }
        journal::replay(&tx, token, query)?;
        storage::evict(&tx)?;
        tx.commit().map_err(sql_error)
    }
}
