//! Small accumulators filled during the single pass over the base.
use super::super::write::{write_all_at, Section};
use super::super::{initial_slot, INITIAL_DEPTH, INITIAL_ENTRY, INITIAL_KEYS, OTHER_EXT};
use super::Key;
use crate::FileRecord;
use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap},
    fs::File,
    io,
};

pub(super) fn align8(value: u64) -> u64 {
    value.div_ceil(8) * 8
}

pub(super) fn name_key(name: &str) -> Key {
    let mut key = [0u8; 16];
    for (slot, byte) in key.iter_mut().zip(name.bytes()) {
        *slot = byte.to_ascii_lowercase();
    }
    key
}

pub(super) struct Initials {
    heaps: Vec<BinaryHeap<Reverse<(u32, u32)>>>,
}

impl Initials {
    pub(super) fn new() -> Self {
        Self {
            heaps: (0..INITIAL_KEYS).map(|_| BinaryHeap::new()).collect(),
        }
    }

    /// Keep the newest INITIAL_DEPTH ids per first character.
    pub(super) fn offer(&mut self, name: &str, mtime: u32, id: u32) {
        let Some(slot) = name.chars().next().and_then(initial_slot) else {
            return;
        };
        let heap = &mut self.heaps[slot];
        let entry = Reverse((mtime, id));
        if heap.len() < INITIAL_DEPTH {
            heap.push(entry);
        } else if heap.peek().is_some_and(|oldest| entry < *oldest) {
            heap.pop();
            heap.push(entry);
        }
    }
}

#[derive(Default)]
pub(super) struct Extensions {
    ids: HashMap<Vec<u8>, u16>,
    ordered: Vec<Box<str>>,
    overflow: bool,
}

impl Extensions {
    pub(super) fn count(&self) -> usize {
        self.ordered.len()
    }

    pub(super) fn overflowed(&self) -> bool {
        self.overflow
    }

    pub(super) fn id(&mut self, record: &FileRecord) -> u16 {
        let ext = record.extension().as_bytes();
        let mut folded = [0u8; 64];
        if ext.len() > folded.len() {
            self.overflow = true;
            return OTHER_EXT;
        }
        for (slot, byte) in folded.iter_mut().zip(ext) {
            *slot = byte.to_ascii_lowercase();
        }
        let folded = &folded[..ext.len()];
        if let Some(id) = self.ids.get(folded) {
            return *id;
        }
        if self.ordered.len() >= usize::from(OTHER_EXT) {
            self.overflow = true;
            return OTHER_EXT;
        }
        let id = self.ordered.len() as u16;
        self.ids.insert(folded.to_vec(), id);
        self.ordered.push(
            String::from_utf8_lossy(folded)
                .into_owned()
                .into_boxed_str(),
        );
        id
    }
}

pub(super) struct Tail {
    pub(super) ext_at: u64,
    pub(super) total: u64,
}

/// Initial-letter lists, their table and the extension names.
pub(super) fn write_tail(
    file: &File,
    lists_at: u64,
    table_at: u64,
    initials: &Initials,
    exts: &Extensions,
) -> io::Result<Tail> {
    let mut lists = Section::new(file, lists_at);
    let mut table = Vec::with_capacity(INITIAL_KEYS * INITIAL_ENTRY);
    let mut cursor = lists_at;
    for heap in &initials.heaps {
        let mut newest: Vec<(u32, u32)> = heap.iter().map(|Reverse(entry)| *entry).collect();
        newest.sort_unstable_by(|a, b| b.cmp(a));
        table.extend_from_slice(&cursor.to_le_bytes());
        table.extend_from_slice(&(newest.len() as u32).to_le_bytes());
        table.extend_from_slice(&0u32.to_le_bytes());
        for (_, id) in &newest {
            lists.push(&id.to_le_bytes())?;
        }
        cursor += 4 * newest.len() as u64;
    }
    write_all_at(file, &table, table_at)?;
    let ext_at = align8(lists.finish()?);
    let mut section = Section::new(file, ext_at);
    for ext in &exts.ordered {
        section.push(&[ext.len() as u8])?;
        section.push(ext.as_bytes())?;
    }
    Ok(Tail {
        ext_at,
        total: section.finish()?,
    })
}
