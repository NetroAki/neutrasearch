use super::cursor_clause;
use crate::browse::columns::{BlockReader, Boundary, Order};
use crate::browse::ordered::newest::Newest;
use crate::browse::ordered::{anchor_boundary, Stream};
use crate::browse::{short_counts, sql_error, BrowserIndex};
use crate::{CompactIndex, FileRecord, MatchFields, Query, SearchHit, SearchStats, SortKey};
use rusqlite::types::Value;
use std::{io, path::Path, time::Instant};

const MAX_CANDIDATES: usize = 200_000;
const PREFIX_SCORE: u32 = (1 << 20) + 256;
const EXACT_SCORE: u32 = (1 << 24) + PREFIX_SCORE;

fn term(query: &Query) -> Option<&str> {
    let [term] = query.terms.as_slice() else {
        return None;
    };
    if query.match_fields != MatchFields::Name
        || query.regex.is_some()
        || query.case_sensitive
        || query.whole_word
        || query.fold_accents
        || !query.kinds.is_empty()
        || !query.exts.is_empty()
        || !query.fss.is_empty()
        || query.executable_only
        || query.min_size.is_some()
        || query.max_size.is_some()
        || query.under.is_some()
        || !query.exclude_roots.is_empty()
        || (!query.scope_roots.is_empty() && query.scope_roots != ["/"])
        || query.limit == 0
        || query.limit > 4096
        || term
            .bytes()
            .any(|b| matches!(b.to_ascii_lowercase(), b'i' | b'k'))
        || short_counts::key(term).is_none()
    {
        return None;
    }
    Some(term)
}

impl BrowserIndex {
    pub fn has_short_search(&self, query: &Query) -> io::Result<bool> {
        if term(query).is_none() {
            return Ok(false);
        }
        Ok(short_counts::ready(&self.db)?.is_some_and(|unix| query.scope_roots.is_empty() || unix))
    }

    /// Short-term totals are maintained on disk; candidate reads have a fixed ceiling.
    pub fn search_short(
        &self,
        index: &Path,
        query: &Query,
        after: Option<&FileRecord>,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> io::Result<Option<(Vec<SearchHit>, SearchStats)>> {
        if cancelled() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "search superseded",
            ));
        }
        let Some(term) = term(query) else {
            return Ok(None);
        };
        if CompactIndex::generation_on_disk(index)? != self.generation {
            return Ok(None);
        }
        let started = Instant::now();
        let snapshot = self.read_snapshot()?;
        if !self.has_short_search(query)? || !self.covers_delta(index)? {
            return Ok(None);
        }
        let matched = short_counts::matched(&snapshot, short_counts::key(term).unwrap())?;
        let hits = if matched == 0 {
            Some(Vec::new())
        } else if query.sort == SortKey::Relevance {
            self.short_relevance(query, term, after, cancelled)?
        } else {
            self.short_ordered(query, term, after, cancelled)?
        };
        Ok(hits.map(|hits| {
            (
                hits,
                SearchStats {
                    scanned: 0,
                    matched,
                    wall_us: started.elapsed().as_micros() as u64,
                },
            )
        }))
    }

    fn short_ordered(
        &self,
        query: &Query,
        term: &str,
        after: Option<&FileRecord>,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> io::Result<Option<Vec<SearchHit>>> {
        let matcher = query.matcher()?;
        let mut stream = Stream::new(self, query, after, None, query.limit.saturating_mul(8))?;
        let mut hits = Vec::with_capacity(query.limit);
        let mut scanned = 0usize;
        while hits.len() < query.limit {
            let Some(record) = stream.next()? else {
                break;
            };
            scanned += 1;
            if scanned % 128 == 1 && cancelled() {
                return Err(superseded());
            }
            if contains(record.name(), term) {
                hits.push(hit(&matcher, record)?);
            }
            if scanned >= MAX_CANDIDATES {
                break;
            }
        }
        Ok((hits.len() == query.limit || scanned < MAX_CANDIDATES).then_some(hits))
    }

    fn short_relevance(
        &self,
        query: &Query,
        term: &str,
        after: Option<&FileRecord>,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> io::Result<Option<Vec<SearchHit>>> {
        let matcher = query.matcher()?;
        let score = after.and_then(|record| matcher.score(record));
        if after.is_some() && !matches!(score, Some(EXACT_SCORE | PREFIX_SCORE)) {
            return Ok(None);
        }
        let dead = self.tombstones()?;
        let mut reader = BlockReader::new(&self.base);
        let limit = query.limit;
        let resume = |reader: &mut BlockReader<'_>| -> io::Result<Option<Boundary>> {
            after
                .map(|anchor| anchor_boundary(&self.cols, reader, Order::Mtime, true, anchor))
                .transpose()
        };
        let (exact_low, exact_high) = self.cols.name_range(&mut reader, term)?;
        let mut hits = Vec::new();
        if score != Some(PREFIX_SCORE) {
            let boundary = resume(&mut reader)?;
            let ids = self.newest_in(exact_low, exact_high, limit, boundary, &dead);
            let mut values = vec![Value::Text(term.to_owned())];
            let mut condition = "name COLLATE NOCASE=?".to_owned();
            if let Some(record) = after {
                condition.push_str(&format!(
                    " AND {}",
                    cursor_clause(SortKey::Relevance, record, &mut values)
                ));
            }
            let sql = format!("SELECT record FROM entries INDEXED BY by_name WHERE {condition} ORDER BY mtime DESC,path DESC LIMIT {limit}");
            let live = self.live_with(&sql, values)?;
            self.collect(
                &matcher,
                &mut reader,
                ids,
                live,
                limit,
                |_| true,
                &mut hits,
                cancelled,
            )?;
        }
        if hits.len() == limit {
            return Ok(Some(hits));
        }
        let remaining = limit - hits.len();
        let (_, prefix_high) = self.cols.prefix_range(&mut reader, term)?;
        let boundary = if score == Some(PREFIX_SCORE) {
            resume(&mut reader)?
        } else {
            None
        };
        let ids = self.newest_in(
            exact_high,
            prefix_high.max(exact_high),
            remaining,
            boundary,
            &dead,
        );
        let mut values = vec![Value::Text(term[..1].to_owned())];
        let mut condition = "substr(name,1,1) COLLATE NOCASE=?".to_owned();
        if score == Some(PREFIX_SCORE) {
            condition.push_str(&format!(
                " AND {}",
                cursor_clause(SortKey::Relevance, after.unwrap(), &mut values)
            ));
        }
        let sql = format!("SELECT record FROM entries INDEXED BY by_initial WHERE {condition} ORDER BY mtime DESC,path DESC LIMIT {MAX_CANDIDATES}");
        let live: Vec<FileRecord> = self
            .live_with(&sql, values)?
            .into_iter()
            .filter(|record| {
                starts_with(record.name(), term) && !record.name().eq_ignore_ascii_case(term)
            })
            .take(remaining)
            .collect();
        self.collect(
            &matcher,
            &mut reader,
            ids,
            live,
            limit,
            |_| true,
            &mut hits,
            cancelled,
        )?;
        Ok((hits.len() == limit).then_some(hits))
    }

    #[allow(clippy::too_many_arguments)]
    fn collect(
        &self,
        matcher: &crate::QueryMatcher,
        reader: &mut BlockReader<'_>,
        ids: Vec<u32>,
        live: Vec<FileRecord>,
        limit: usize,
        keep: impl Fn(&FileRecord) -> bool,
        hits: &mut Vec<SearchHit>,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> io::Result<()> {
        // Both lists already hold at least the requested count when cut.
        let mut newest = Newest::new(ids, false, live, false);
        while hits.len() < limit {
            if cancelled() {
                return Err(superseded());
            }
            let Some(record) = newest.next(reader)? else {
                break;
            };
            if keep(&record) {
                hits.push(hit(matcher, record)?);
            }
        }
        Ok(())
    }

    fn live_with(&self, sql: &str, values: Vec<Value>) -> io::Result<Vec<FileRecord>> {
        let mut statement = self.db.prepare(sql).map_err(sql_error)?;
        let mut rows = statement
            .query(rusqlite::params_from_iter(values))
            .map_err(sql_error)?;
        let mut found = Vec::new();
        while let Some(row) = rows.next().map_err(sql_error)? {
            let bytes: Vec<u8> = row.get(0).map_err(sql_error)?;
            found.push(bincode::deserialize(&bytes).map_err(io::Error::other)?);
        }
        Ok(found)
    }
}

fn superseded() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "search superseded")
}

fn contains(name: &str, term: &str) -> bool {
    name.as_bytes()
        .windows(term.len())
        .any(|part| part.eq_ignore_ascii_case(term.as_bytes()))
}

fn starts_with(name: &str, term: &str) -> bool {
    name.as_bytes()
        .get(..term.len())
        .is_some_and(|start| start.eq_ignore_ascii_case(term.as_bytes()))
}

fn hit(matcher: &crate::QueryMatcher, record: FileRecord) -> io::Result<SearchHit> {
    let score = matcher
        .score(&record)
        .ok_or_else(|| io::Error::other("short search matcher disagrees with catalog"))?;
    Ok(SearchHit { score, record })
}
