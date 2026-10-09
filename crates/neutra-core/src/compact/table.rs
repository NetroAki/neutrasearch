//! Fixed-width tables (block directory, trigram dictionary) read straight from
//! the mapped index file on demand, so they cost no heap.

use super::{u16_at, u32_at, u64_at, BlockDesc, CompactIndex, DictEntry, DESC_SIZE, DICT_SIZE};

#[derive(Clone, Copy)]
pub(super) struct TableView {
    pub(super) start: usize,
    pub(super) count: usize,
}

impl CompactIndex {
    pub(crate) fn block_count(&self) -> usize {
        self.blocks.count
    }

    pub(super) fn block(&self, id: usize) -> Option<BlockDesc> {
        if id >= self.blocks.count {
            return None;
        }
        let p = self.blocks.start + id * DESC_SIZE as usize;
        Some(BlockDesc {
            offset: u64_at(&self.map, p).ok()?,
            len: u32_at(&self.map, p + 8).ok()?,
            count: u16_at(&self.map, p + 12).ok()?,
        })
    }

    fn dict_entry(&self, i: usize) -> Option<DictEntry> {
        let p = self.dict.start + i * DICT_SIZE as usize;
        Some(DictEntry {
            gram: u32_at(&self.map, p).ok()?,
            len: u32_at(&self.map, p + 4).ok()?,
            offset: u64_at(&self.map, p + 8).ok()?,
        })
    }

    /// Binary search over the sorted dictionary in the mapped file.
    pub(super) fn dict_find(&self, gram: u32) -> Option<DictEntry> {
        let (mut low, mut high) = (0, self.dict.count);
        while low < high {
            let mid = low + (high - low) / 2;
            let entry = self.dict_entry(mid)?;
            match entry.gram.cmp(&gram) {
                std::cmp::Ordering::Equal => return Some(entry),
                std::cmp::Ordering::Less => low = mid + 1,
                std::cmp::Ordering::Greater => high = mid,
            }
        }
        None
    }
}
