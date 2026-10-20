//! Newest-first merging of base ids and live rows for filename recall.
use crate::browse::catalog::Tombstones;
use crate::browse::columns::{BlockReader, Order, Span, INITIAL_DEPTH};
use crate::browse::{sql_error, BrowserIndex};
use crate::FileRecord;
use std::{cmp::Ordering, io, vec::IntoIter};

pub(in crate::browse) const CAP: usize = 4096;

fn stamp(record: &FileRecord) -> i64 {
    if record.mtime > 4_102_444_800 {
        0
    } else {
        record.mtime
    }
}

/// Two candidate lists, each sorted newest first (mtime, then path, descending).
/// A list cut at its cap may hide older entries, so reaching its end ends the
/// stream as incomplete instead of letting the other list run ahead.
pub(in crate::browse) struct Newest {
    base: IntoIter<u32>,
    base_cut: bool,
    base_head: Option<FileRecord>,
    base_done: bool,
    live: IntoIter<FileRecord>,
    live_cut: bool,
    live_head: Option<FileRecord>,
    live_done: bool,
    pub(in crate::browse) incomplete: bool,
}

impl Newest {
    pub(in crate::browse) fn new(
        base: Vec<u32>,
        base_cut: bool,
        live: Vec<FileRecord>,
        live_cut: bool,
    ) -> Self {
        Self {
            base: base.into_iter(),
            base_cut,
            base_head: None,
            base_done: false,
            live: live.into_iter(),
            live_cut,
            live_head: None,
            live_done: false,
            incomplete: false,
        }
    }

    pub(in crate::browse) fn next(
        &mut self,
        reader: &mut BlockReader<'_>,
    ) -> io::Result<Option<FileRecord>> {
        if self.base_head.is_none() && !self.base_done {
            match self.base.next() {
                Some(id) => self.base_head = Some(reader.record(id)?.clone()),
                None => self.base_done = true,
            }
        }
        if self.live_head.is_none() && !self.live_done {
            self.live_head = self.live.next();
            self.live_done = self.live_head.is_none();
        }
        if (self.base_done && self.base_cut) || (self.live_done && self.live_cut) {
            self.incomplete = true;
            return Ok(None);
        }
        let take_base = match (&self.base_head, &self.live_head) {
            (None, None) => return Ok(None),
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (Some(base), Some(live)) => {
                (stamp(base), &base.path).cmp(&(stamp(live), &live.path)) == Ordering::Greater
            }
        };
        Ok(if take_base {
            self.base_head.take()
        } else {
            self.live_head.take()
        })
    }
}

impl BrowserIndex {
    /// Live base ids whose names equal the term, newest first.
    pub(in crate::browse) fn base_equal(
        &self,
        reader: &mut BlockReader<'_>,
        dead: &Tombstones,
        term: &str,
        cap: usize,
    ) -> io::Result<Vec<u32>> {
        let (low, high) = self.cols.name_range(reader, term)?;
        Ok(self.newest_in(low, high, cap, None, dead))
    }

    pub(in crate::browse) fn newest_in(
        &self,
        low: usize,
        high: usize,
        cap: usize,
        after: Option<crate::browse::columns::Boundary>,
        dead: &Tombstones,
    ) -> Vec<u32> {
        self.cols
            .top(
                &[Span::Perm(low, high)],
                Order::Mtime,
                true,
                cap,
                after,
                &|id| !dead.contains(id),
            )
            .into_iter()
            .map(|(_, id)| id)
            .collect()
    }

    /// Newest live base ids sharing an initial, and whether the list was cut.
    pub(in crate::browse) fn base_initial(
        &self,
        slot: usize,
        dead: &Tombstones,
    ) -> (Vec<u32>, bool) {
        let all = self.cols.initial_list(slot);
        let cut = all.len() >= INITIAL_DEPTH;
        (
            all.into_iter().filter(|id| !dead.contains(*id)).collect(),
            cut,
        )
    }

    pub(in crate::browse) fn live_equal(
        &self,
        term: &str,
        cap: usize,
    ) -> io::Result<Vec<FileRecord>> {
        self.live_rows(
            "SELECT record FROM entries INDEXED BY by_quick WHERE name COLLATE NOCASE=?1 ORDER BY mtime DESC,path DESC LIMIT ?2",
            term,
            cap,
        )
    }

    pub(in crate::browse) fn live_initial(
        &self,
        initial: &str,
        cap: usize,
    ) -> io::Result<Vec<FileRecord>> {
        self.live_rows(
            "SELECT record FROM entries INDEXED BY by_initial WHERE substr(name,1,1) COLLATE NOCASE=?1 ORDER BY mtime DESC,path DESC LIMIT ?2",
            initial,
            cap,
        )
    }

    fn live_rows(&self, sql: &str, key: &str, cap: usize) -> io::Result<Vec<FileRecord>> {
        let mut statement = self.db.prepare_cached(sql).map_err(sql_error)?;
        let mut rows = statement
            .query(rusqlite::params![key, cap as i64])
            .map_err(sql_error)?;
        let mut found = Vec::new();
        while let Some(row) = rows.next().map_err(sql_error)? {
            let bytes: Vec<u8> = row.get(0).map_err(sql_error)?;
            found.push(bincode::deserialize(&bytes).map_err(io::Error::other)?);
        }
        Ok(found)
    }
}
