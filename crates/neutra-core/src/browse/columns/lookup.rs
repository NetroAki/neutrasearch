//! Decoded-record access for ids and ordered probes into the pinned base.
use super::Columns;
use crate::compact::compare_index_paths;
use crate::{CompactIndex, FileRecord};
use std::{
    cmp::Ordering,
    collections::VecDeque,
    io,
    sync::{Arc, Mutex},
};

#[cfg(test)]
mod cache_tests;

const HELD: usize = 16;
pub(super) const DECODED_BLOCK_LIMIT: usize = 128;
pub(super) const DECODED_BYTE_LIMIT: usize = 8 * 1024 * 1024;

/// Bounded LRU of decoded blocks from one immutable compact base.
#[derive(Default)]
pub(in crate::browse) struct DecodedBlocks {
    entries: VecDeque<(u32, Arc<Vec<FileRecord>>, usize)>,
    bytes: usize,
}

impl DecodedBlocks {
    pub(in crate::browse) fn new() -> Self {
        Self::default()
    }
}

impl DecodedBlocks {
    fn get_or_load(
        &mut self,
        block: u32,
        load: impl FnOnce() -> io::Result<Vec<FileRecord>>,
    ) -> io::Result<Arc<Vec<FileRecord>>> {
        if let Some(at) = self.entries.iter().position(|(held, _, _)| *held == block) {
            let entry = self.entries.remove(at).expect("located cache entry");
            let records = Arc::clone(&entry.1);
            self.entries.push_back(entry);
            return Ok(records);
        }
        let records = Arc::new(load()?);
        let bytes = records
            .iter()
            .map(|r| std::mem::size_of::<FileRecord>() + r.path.len())
            .sum();
        if bytes > DECODED_BYTE_LIMIT {
            return Ok(records);
        }
        while self.entries.len() >= DECODED_BLOCK_LIMIT
            || self.bytes.saturating_add(bytes) > DECODED_BYTE_LIMIT
        {
            let Some((_, _, removed)) = self.entries.pop_front() else {
                break;
            };
            self.bytes -= removed;
        }
        self.bytes += bytes;
        self.entries.push_back((block, Arc::clone(&records), bytes));
        Ok(records)
    }
}

/// Small cache of decoded 32-record blocks; neighbouring ids share a block.
pub(in crate::browse) struct BlockReader<'a> {
    base: &'a CompactIndex,
    held: Vec<(u32, Arc<Vec<FileRecord>>)>,
    cache: Option<&'a Mutex<DecodedBlocks>>,
}

impl<'a> BlockReader<'a> {
    pub(in crate::browse) fn new(base: &'a CompactIndex) -> Self {
        Self {
            base,
            held: Vec::with_capacity(HELD),
            cache: None,
        }
    }

    pub(in crate::browse) fn with_cache(
        base: &'a CompactIndex,
        cache: &'a Mutex<DecodedBlocks>,
    ) -> Self {
        Self {
            base,
            held: Vec::with_capacity(HELD),
            cache: Some(cache),
        }
    }

    pub(in crate::browse) fn base(&self) -> &'a CompactIndex {
        self.base
    }

    fn slot(&mut self, block: u32) -> io::Result<usize> {
        if let Some(at) = self.held.iter().position(|(held, _)| *held == block) {
            return Ok(at);
        }
        let records = if let Some(cache) = self.cache {
            cache
                .lock()
                .map_err(|_| io::Error::other("decoded block cache poisoned"))?
                .get_or_load(block, || {
                    count_decode();
                    let records = self.base.read_block(block)?;
                    Ok(records)
                })?
        } else {
            count_decode();
            let records = self.base.read_block(block)?;
            Arc::new(records)
        };
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
        self.base.require_current_order()?;
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

#[cfg(test)]
thread_local! { static DECODE_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

#[inline]
fn count_decode() {
    #[cfg(test)]
    DECODE_COUNT.with(|count| count.set(count.get() + 1));
}

#[cfg(test)]
pub(super) fn decode_count() -> usize {
    DECODE_COUNT.with(std::cell::Cell::get)
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
