//! Unicode equivalents for the two ASCII initials SQLite NOCASE misses.
use crate::browse::columns::{initial_slot, BlockReader};
use crate::browse::ordered::newest::{Newest, CAP};
use crate::browse::BrowserIndex;
use crate::{DeltaIndex, Query, SearchHit};
use std::io;

impl BrowserIndex {
    pub(super) fn quick_letter(
        &self,
        query: &Query,
        term: &str,
        delta: Option<&DeltaIndex>,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> io::Result<Option<Vec<SearchHit>>> {
        let variants = if term.eq_ignore_ascii_case("i") {
            ["i", "\u{130}"]
        } else {
            ["k", "\u{212A}"]
        };
        let mut hits = Vec::new();
        for variant in variants {
            let Some(exact) = self.quick_exact(query, variant, delta, cancelled)? else {
                return Ok(None);
            };
            hits.extend(exact);
        }
        if hits.len() < query.limit {
            for variant in variants {
                let Some(prefixes) = self.letter_prefix(query, variant, delta, cancelled)? else {
                    return Ok(None);
                };
                hits.extend(prefixes);
            }
        }
        hits.sort_unstable_by(|a, b| {
            crate::matcher::compare_records(
                query.sort,
                &(a.score, &a.record),
                &(b.score, &b.record),
            )
        });
        hits.truncate(query.limit);
        Ok((hits.len() == query.limit).then_some(hits))
    }

    fn letter_prefix(
        &self,
        query: &Query,
        initial: &str,
        delta: Option<&DeltaIndex>,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> io::Result<Option<Vec<SearchHit>>> {
        let matcher = query.matcher()?;
        let dead = self.tombstones()?;
        let mut reader = BlockReader::new(&self.base);
        let slot = initial.chars().next().and_then(initial_slot);
        let (ids, cut) = match slot {
            Some(slot) => self.base_initial(slot, &dead),
            None => (Vec::new(), false),
        };
        let live = self.live_initial(initial, CAP)?;
        let live_cut = live.len() == CAP;
        let mut newest = Newest::new(ids, cut, live, live_cut);
        let mut hits = Vec::new();
        while hits.len() < query.limit {
            if cancelled() {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "search superseded",
                ));
            }
            let Some(record) = newest.next(&mut reader)? else {
                break;
            };
            if record.name().chars().count() == 1 || super::delta::shadowed(delta, &record.path)? {
                continue;
            }
            if let Some(score) = matcher.score(&record) {
                hits.push(SearchHit { score, record });
            }
        }
        if hits.len() < query.limit && newest.incomplete {
            return Ok(None);
        }
        Ok(Some(hits))
    }
}
