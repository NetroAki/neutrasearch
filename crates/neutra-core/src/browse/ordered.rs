//! One globally ordered stream over every live source of the catalog: base
//! rows from the flat columns, changed rows from the SQLite table, and the
//! unapplied log. Callers pull records in sort order and stop when satisfied.

mod base;
mod live;
pub(super) mod newest;

use super::catalog::Tombstones;
use super::columns::BlockReader;
use super::BrowserIndex;
use crate::matcher::compare_records;
use crate::{DeltaIndex, FileRecord, Query};
pub(super) use base::anchor_boundary;
use base::BaseSide;
use live::LiveSide;
use std::{cmp::Ordering, io, sync::Arc};

pub(super) struct Stream<'a> {
    browser: &'a BrowserIndex,
    query: &'a Query,
    delta: Option<&'a DeltaIndex>,
    reader: BlockReader<'a>,
    dead: Arc<Tombstones>,
    base: BaseSide,
    live: LiveSide,
    log: std::vec::IntoIter<FileRecord>,
    heads: [Option<FileRecord>; 3],
    primed: bool,
    // Holds one read snapshot so tombstones and live rows agree.
    _snapshot: Option<rusqlite::Transaction<'a>>,
}

impl<'a> Stream<'a> {
    /// `hint` is roughly how many records the caller expects to pull.
    pub(super) fn new(
        browser: &'a BrowserIndex,
        query: &'a Query,
        after: Option<&FileRecord>,
        delta: Option<&'a DeltaIndex>,
        hint: usize,
    ) -> io::Result<Self> {
        let snapshot = if browser.db.is_autocommit() {
            Some(browser.read_snapshot()?)
        } else {
            None
        };
        browser.require_pinned_base()?;
        let dead = browser.tombstones()?;
        let mut reader = BlockReader::new(&browser.base);
        let base = BaseSide::new(&browser.cols, &mut reader, query, after, hint)?;
        let mut log = Vec::new();
        if let Some(delta) = delta {
            delta.for_each_upsert(|record| {
                let past_anchor = after.is_none_or(|anchor| {
                    compare_records(query.sort, &(0, &record), &(0, anchor)) == Ordering::Greater
                });
                if past_anchor && query.passes_filters(&record) {
                    log.push(record);
                }
                Ok(())
            })?;
            log.sort_unstable_by(|a, b| compare_records(query.sort, &(0, a), &(0, b)));
        }
        Ok(Self {
            browser,
            query,
            delta,
            reader,
            dead,
            base,
            live: LiveSide::new(query, after),
            log: log.into_iter(),
            heads: [None, None, None],
            primed: false,
            _snapshot: snapshot,
        })
    }

    fn eligible(&self, record: &FileRecord) -> io::Result<bool> {
        if let Some(delta) = self.delta {
            if delta.shadows(&record.path)? {
                return Ok(false);
            }
        }
        Ok(self.query.passes_filters(record))
    }

    fn fill(&mut self, side: usize) -> io::Result<()> {
        self.heads[side] = None;
        loop {
            let candidate = match side {
                0 => match self.base.next_id(&self.browser.cols, &self.dead) {
                    Some(id) => Some(self.reader.record(id)?.clone()),
                    None => None,
                },
                1 => self.live.next_record(&self.browser.db)?,
                _ => self.log.next(),
            };
            let Some(record) = candidate else {
                return Ok(());
            };
            if side == 2 || self.eligible(&record)? {
                self.heads[side] = Some(record);
                return Ok(());
            }
        }
    }

    /// The next record in sort order, or None once every source is drained.
    pub(super) fn next(&mut self) -> io::Result<Option<FileRecord>> {
        if !self.primed {
            for side in 0..3 {
                self.fill(side)?;
            }
            self.primed = true;
        }
        let mut best: Option<usize> = None;
        for side in 0..3 {
            let Some(head) = &self.heads[side] else {
                continue;
            };
            let better = match best.and_then(|at| self.heads[at].as_ref()) {
                None => true,
                Some(current) => {
                    compare_records(self.query.sort, &(0, head), &(0, current)) == Ordering::Less
                }
            };
            if better {
                best = Some(side);
            }
        }
        let Some(side) = best else {
            return Ok(None);
        };
        let record = self.heads[side].take();
        self.fill(side)?;
        Ok(record)
    }
}
