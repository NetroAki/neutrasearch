//! Decoded-record access for ids and ordered probes into the pinned base.
use super::Columns;
use crate::compact::compare_index_paths;
use crate::{CompactIndex, FileRecord};
use std::{cmp::Ordering, io};

const HELD: usize = 16;

/// Small cache of decoded 32-record blocks; neighbouring ids share a block.
pub(in crate::browse) struct BlockReader<'a> {
    base: &'a CompactIndex,
    held: Vec<(u32, Vec<FileRecord>)>,
}

impl<'a> BlockReader<'a> {
    pub(in crate::browse) fn new(base: &'a CompactIndex) -> Self {
        Self {
            base,
            held: Vec::with_capacity(HELD),
        }
    }

    pub(in crate::browse) fn base(&self) -> &'a CompactIndex {
        self.base
    }

    fn slot(&mut self, block: u32) -> io::Result<usize> {
        if let Some(at) = self.held.iter().position(|(held, _)| *held == block) {
            return Ok(at);
        }
        let records = self.base.read_block(block)?;
        self.base.release_block(block);
        if self.held.len() == HELD {
            self.held.remove(0);
        }
        self.held.push((block, records));
        Ok(self.held.len() - 1)
    }

    pub(in crate::browse) fn record(&mut self, id: u32) -> io::Result<&FileRecord> {
        let starts = self.base.block_starts();
        let invalid = || {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "invalid browse record id {id} of {} records",
                    self.base.len()
                ),
            )
        };
        if starts.last().is_none_or(|total| id >= *total) {
            return Err(invalid());
        }
        let block = starts.partition_point(|&start| start <= id) - 1;
        let slot = (id - starts[block]) as usize;
        let at = self.slot(block as u32)?;
        self.held[at].1.get(slot).ok_or_else(invalid)
    }

    /// First id whose path is not before the argument in base order.
    pub(in crate::browse) fn path_lower_bound(&mut self, path: &str) -> io::Result<u32> {
        let blocks = self.base.block_count();
        let (mut low, mut high) = (0usize, blocks);
        while low < high {
            let middle = low + (high - low) / 2;
            let at = self.slot(middle as u32)?;
            let before = self.held[at]
                .1
                .last()
                .is_some_and(|last| compare_index_paths(&last.path, path) == Ordering::Less);
            if before {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        if low == blocks {
            return Ok(self.base.len() as u32);
        }
        let at = self.slot(low as u32)?;
        let inside = self.held[at]
            .1
            .partition_point(|r| compare_index_paths(&r.path, path) == Ordering::Less);
        Ok(self.base.block_starts()[low] + inside as u32)
    }
}

impl Columns {
    /// First name-order position for which the predicate is false. The
    /// predicate must hold for a prefix of the order and fail afterwards.
    pub(in crate::browse) fn perm_partition(
        &self,
        reader: &mut BlockReader<'_>,
        mut before: impl FnMut(&FileRecord) -> bool,
    ) -> io::Result<usize> {
        let (mut low, mut high) = (0usize, self.len());
        while low < high {
            let middle = low + (high - low) / 2;
            let id = self.perm(middle);
            if before(reader.record(id)?) {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        Ok(low)
    }

    /// Positions holding names equal (case-insensitively) to the term.
    pub(in crate::browse) fn name_range(
        &self,
        reader: &mut BlockReader<'_>,
        term: &str,
    ) -> io::Result<(usize, usize)> {
        use crate::matcher::cmp_name_ci;
        let low = self.perm_partition(reader, |r| cmp_name_ci(r.name(), term) == Ordering::Less)?;
        let high =
            self.perm_partition(reader, |r| cmp_name_ci(r.name(), term) != Ordering::Greater)?;
        Ok((low, high))
    }

    /// Positions holding names that start with the term, ignoring ASCII case.
    pub(in crate::browse) fn prefix_range(
        &self,
        reader: &mut BlockReader<'_>,
        term: &str,
    ) -> io::Result<(usize, usize)> {
        use crate::matcher::cmp_name_ci;
        let starts = |name: &str| {
            name.as_bytes()
                .get(..term.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(term.as_bytes()))
        };
        let low = self.perm_partition(reader, |r| cmp_name_ci(r.name(), term) == Ordering::Less)?;
        let high = self.perm_partition(reader, |r| {
            starts(r.name()) || cmp_name_ci(r.name(), term) == Ordering::Less
        })?;
        Ok((low, high))
    }
}
