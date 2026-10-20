//! Sorted runs and their k-way merge produce the name-order permutation.
use super::super::write::{read_exact_at, write_all_at, Section};
use super::super::BlockReader;
use crate::CompactIndex;
use std::{cmp::Reverse, collections::BinaryHeap, fs::File, io};

pub(super) const RUN_ROWS: usize = 2 << 20;
const ROW: usize = 20;
const READ_BUFFER: usize = 64 << 10;

pub(super) type Key = [u8; 16];

pub(super) fn flush_run(
    rows: &mut Vec<(Key, u32)>,
    runs: &File,
    written: &mut u64,
) -> io::Result<()> {
    rayon::slice::ParallelSliceMut::par_sort_unstable(rows.as_mut_slice());
    let mut bytes = Vec::with_capacity(rows.len() * ROW);
    for (key, id) in rows.iter() {
        bytes.extend_from_slice(key);
        bytes.extend_from_slice(&id.to_le_bytes());
    }
    write_all_at(runs, &bytes, *written)?;
    *written += bytes.len() as u64;
    rows.clear();
    Ok(())
}

struct RunReader<'a> {
    file: &'a File,
    next: u64,
    end: u64,
    buffer: Vec<u8>,
    at: usize,
}

impl RunReader<'_> {
    fn pop(&mut self) -> io::Result<Option<(Key, u32)>> {
        if self.at == self.buffer.len() {
            if self.next >= self.end {
                return Ok(None);
            }
            let take = ((self.end - self.next) as usize).min(READ_BUFFER / ROW * ROW);
            self.buffer.resize(take, 0);
            read_exact_at(self.file, &mut self.buffer, self.next)?;
            self.next += take as u64;
            self.at = 0;
        }
        let row = &self.buffer[self.at..self.at + ROW];
        self.at += ROW;
        let id = u32::from_le_bytes(row[16..].try_into().unwrap());
        Ok(Some((row[..16].try_into().unwrap(), id)))
    }
}

pub(super) fn merge_runs(
    base: &CompactIndex,
    runs: &File,
    ends: &[u64],
    perm: &mut Section<'_>,
) -> io::Result<()> {
    let mut readers = Vec::new();
    let mut start = 0;
    for end in ends {
        readers.push(RunReader {
            file: runs,
            next: start,
            end: *end,
            buffer: Vec::new(),
            at: 0,
        });
        start = *end;
    }
    let mut heap = BinaryHeap::new();
    for (index, reader) in readers.iter_mut().enumerate() {
        if let Some((key, id)) = reader.pop()? {
            heap.push(Reverse((key, id, index)));
        }
    }
    let mut reader = BlockReader::new(base);
    let (mut group, mut group_key): (Vec<u32>, Key) = (Vec::new(), [0; 16]);
    while let Some(Reverse((key, id, index))) = heap.pop() {
        if let Some((next, next_id)) = readers[index].pop()? {
            heap.push(Reverse((next, next_id, index)));
        }
        if !group.is_empty() && key != group_key {
            flush_group(&mut reader, &mut group, &group_key, perm)?;
        }
        group_key = key;
        group.push(id);
    }
    flush_group(&mut reader, &mut group, &group_key, perm)
}

/// Equal sixteen-byte keys arrive in id order. They need a full-name sort
/// only when the key is saturated, because longer names may still differ.
fn flush_group(
    reader: &mut BlockReader<'_>,
    group: &mut Vec<u32>,
    key: &Key,
    perm: &mut Section<'_>,
) -> io::Result<()> {
    if group.len() > 1 && key[15] != 0 {
        let mut named = Vec::with_capacity(group.len());
        for id in group.iter() {
            let name = reader.record(*id)?.name().to_ascii_lowercase().into_bytes();
            named.push((name, *id));
        }
        named.sort_unstable();
        group.clear();
        group.extend(named.into_iter().map(|(_, id)| id));
    }
    for id in group.drain(..) {
        perm.push(&id.to_le_bytes())?;
    }
    Ok(())
}
