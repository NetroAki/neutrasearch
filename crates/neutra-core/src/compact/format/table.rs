//! Fixed-width tables (block directory, trigram dictionary) read straight from
//! the mapped index file on demand, so they cost no heap.

use super::{u16_at, u32_at, u64_at, BlockDesc, CompactIndex, DictEntry, DESC_SIZE, DICT_SIZE};

#[derive(Clone, Copy)]
pub(in crate::compact) struct TableView {
    pub(in crate::compact) start: usize,
    pub(in crate::compact) count: usize,
}

impl CompactIndex {
    pub(crate) fn block_count(&self) -> usize {
        self.blocks.count
    }

    /// First record id of every block, plus the total as a final entry.
    /// Compaction can leave short blocks, so ids are not block * 32 + slot.
    pub(crate) fn block_starts(&self) -> &[u32] {
        self.starts.get_or_init(|| {
            let mut starts = Vec::with_capacity(self.blocks.count + 1);
            let mut total = 0u32;
            for id in 0..self.blocks.count {
                starts.push(total);
                total += self.block(id).map_or(0, |desc| u32::from(desc.count));
            }
            starts.push(total);
            starts.into_boxed_slice()
        })
    }

    pub(in crate::compact) fn block(&self, id: usize) -> Option<BlockDesc> {
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

    pub(in crate::compact) fn dict_entry(&self, i: usize) -> Option<DictEntry> {
        let p = self.dict.start + i * DICT_SIZE as usize;
        Some(DictEntry {
            gram: u32_at(&self.map, p).ok()?,
            len: u32_at(&self.map, p + 4).ok()?,
            offset: u64_at(&self.map, p + 8).ok()?,
        })
    }

    /// Binary search over the sorted dictionary in the mapped file.
    pub(in crate::compact) fn dict_find(&self, gram: u32) -> Option<DictEntry> {
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
