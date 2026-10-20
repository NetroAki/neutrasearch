//! First-page filename recall using the existing name and initial-letter orders.

use super::columns::{initial_slot, BlockReader};
use super::ordered::newest::{Newest, CAP};
use super::{sql_error, BrowserIndex};
use crate::{CompactIndex, DeltaIndex, MatchFields, Query, SearchHit, SortKey};
use std::{io, path::Path};

mod delta;
mod letters;

impl BrowserIndex {
    /// Return a proven complete first page without counting every substring hit.
    /// None asks the caller to use the full matcher (including non-prefix hits).
    pub fn search_quick(
        &self,
        index: &Path,
        query: &Query,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> io::Result<Option<Vec<SearchHit>>> {
        self.search_quick_with_delta(index, query, None, cancelled)
    }

    /// Merge indexed candidates with the verified log when the catalog lags.
    pub fn search_quick_with_delta(
        &self,
        index: &Path,
        query: &Query,
        delta: Option<&mut DeltaIndex>,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> io::Result<Option<Vec<SearchHit>>> {
        let [term] = query.terms.as_slice() else {
            return Ok(None);
        };
        let ordinary = Query {
            terms: query.terms.clone(),
            match_fields: MatchFields::Name,
            limit: query.limit,
            ..Query::default()
        };
        if query != &ordinary || query.limit == 0 || query.limit > 100
            || term.is_empty() || !term.is_ascii()
            // These ASCII letters also match Unicode case mappings that
            // SQLite's NOCASE order cannot represent.
            || (term.len() > 1
                && term.bytes().any(|b| matches!(b.to_ascii_lowercase(), b'i' | b'k')))
        {
            return Ok(None);
        }
        let snapshot = self.db.unchecked_transaction().map_err(sql_error)?;
        if CompactIndex::generation_on_disk(index)? != self.generation
            || super::short_counts::ready(&snapshot)? != Some(true)
        {
            return Ok(None);
        }
        let delta = if self.covers_delta(index)? {
            None
        } else {
            let Some(delta) = delta else {
                return Ok(None);
            };
            if !self.delta_covers_catalog(delta)? {
                return Ok(None);
            }
            Some(delta)
        };
        let overlay = delta.as_deref();
        let candidates = if matches!(term.to_ascii_lowercase().as_str(), "i" | "k") {
            self.quick_letter(query, term, overlay, cancelled)?
        } else {
            self.quick_exact(query, term, overlay, cancelled)?
        };
        let Some(mut hits) = candidates else {
            return Ok(None);
        };
        if hits.len() < query.limit {
            let Some(prefixes) = self.quick_prefix(query, term, hits, overlay, cancelled)? else {
                return Ok(None);
            };
            hits = prefixes;
        }
        if let Some(delta) = overlay {
            delta::merge(delta, query, &mut hits, cancelled)?;
        }
        Ok(Some(hits))
    }

    pub(super) fn quick_exact(
        &self,
        query: &Query,
        term: &str,
        delta: Option<&DeltaIndex>,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> io::Result<Option<Vec<SearchHit>>> {
        let matcher = query.matcher()?;
        let dead = self.tombstones()?;
        let mut reader = BlockReader::new(&self.base);
        // Bound both candidate sets. An exhausted bound is a fallback, never
        // an incomplete answer presented as the best matches.
        let ids = self.base_equal(&mut reader, &dead, term, CAP)?;
        let live = self.live_equal(term, CAP)?;
        let (ids_cut, live_cut) = (ids.len() == CAP, live.len() == CAP);
        let mut newest = Newest::new(ids, ids_cut, live, live_cut);
        let mut hits = Vec::with_capacity(query.limit);
        while let Some(record) = newest.next(&mut reader)? {
            if cancelled() {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "search superseded",
                ));
            }
            if delta::shadowed(delta, &record.path)? {
                continue;
            }
            if let Some(score) = matcher.score(&record) {
                hits.push(SearchHit { score, record });
            }
            if hits.len() == query.limit {
                return Ok(Some(hits));
            }
        }
        if newest.incomplete {
            return Ok(None);
        }
        hits.sort_unstable_by(|a, b| {
            crate::matcher::compare_records(
                SortKey::Relevance,
                &(a.score, &a.record),
                &(b.score, &b.record),
            )
        });
        hits.truncate(query.limit);
        Ok(Some(hits))
    }

    fn quick_prefix(
        &self,
        query: &Query,
        term: &str,
        mut hits: Vec<SearchHit>,
        delta: Option<&DeltaIndex>,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> io::Result<Option<Vec<SearchHit>>> {
        let matcher = query.matcher()?;
        let dead = self.tombstones()?;
        let mut reader = BlockReader::new(&self.base);
        let slot = term.chars().next().and_then(initial_slot);
        let (ids, cut) = match slot {
            Some(slot) => self.base_initial(slot, &dead),
            None => (Vec::new(), false),
        };
        let live = self.live_initial(&term[..1], CAP)?;
        let live_cut = live.len() == CAP;
        let mut newest = Newest::new(ids, cut, live, live_cut);
        while let Some(record) = newest.next(&mut reader)? {
            if cancelled() {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "search superseded",
                ));
            }
            let name = record.name();
            if name.eq_ignore_ascii_case(term)
                || !name
                    .as_bytes()
                    .get(..term.len())
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case(term.as_bytes()))
                || delta::shadowed(delta, &record.path)?
            {
                continue;
            }
            if let Some(score) = matcher.score(&record) {
                hits.push(SearchHit { score, record });
            }
            if hits.len() == query.limit {
                return Ok(Some(hits));
            }
        }
        Ok(None)
    }
}
