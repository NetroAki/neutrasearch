//! Sorted runs and k-way merges over spilled chunks. Each pass sorts every
//! chunk by the pass key, writes run files, and streams the merged order;
//! only one head record per run stays resident.

use crate::compact_spill::{read_framed, write_framed, SpillRuns};
use crate::FileRecord;
use rayon::prelude::*;
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::fs::File;
use std::io::{self, BufReader, BufWriter};
use std::path::Path;

mod keys;
mod postings;
use keys::{compare, key_for, MergeKey};
pub(crate) use postings::{sorted_pairs, write_postings_from_pairs, write_postings_sorted};

/// Merge order for a spill pass.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpillOrder {
    /// Compact-base order: path only.
    Compact,
    /// Directory-summary order: source, then path.
    Summary,
}

struct RunReader {
    reader: BufReader<File>,
    head: Option<FileRecord>,
}

/// K-way merge over per-chunk sorted runs.
pub(crate) struct RunMerge {
    readers: Vec<Option<RunReader>>,
    heap: BinaryHeap<Reverse<MergeKey>>,
    order: SpillOrder,
}

impl RunMerge {
    /// Sort every chunk by `order`, write run files, and open the merge.
    /// Run files share the spill directory and vanish with it.
    pub(crate) fn open(runs: &SpillRuns, order: SpillOrder) -> io::Result<Self> {
        let dir = runs.dir()?;
        let tag = match order {
            SpillOrder::Compact => "c",
            SpillOrder::Summary => "s",
        };
        let mut readers: Vec<Option<RunReader>> = (0..runs.chunks().len()).map(|_| None).collect();
        let mut heap = BinaryHeap::new();
        for (index, chunk) in runs.chunks().iter().enumerate() {
            let mut records = read_chunk(chunk)?;
            records.par_sort_by(|left, right| compare(order, left, right));
            let path = dir.join(format!("run-{tag}-{index:06}.srt"));
            write_chunk(&path, &records)?;
            drop(records);
            let mut reader = BufReader::with_capacity(1 << 20, File::open(&path)?);
            if let Some(head) = crate::compact_spill::read_framed(&mut reader)? {
                heap.push(Reverse(key_for(order, &head, index)));
                readers[index] = Some(RunReader {
                    reader,
                    head: Some(head),
                });
            }
        }
        Ok(Self {
            readers,
            heap,
            order,
        })
    }
}

fn read_chunk(path: &Path) -> io::Result<Vec<FileRecord>> {
    let mut reader = BufReader::with_capacity(1 << 20, File::open(path)?);
    let mut records = Vec::new();
    while let Some(record) = read_framed(&mut reader)? {
        records.push(record);
    }
    Ok(records)
}

fn write_chunk(path: &Path, records: &[FileRecord]) -> io::Result<()> {
    let file = crate::compact_spill::create_private_file(path)?;
    let mut writer = BufWriter::with_capacity(1 << 20, file);
    for record in records {
        write_framed(&mut writer, record)?;
    }
    writer.flush()
}

impl Iterator for RunMerge {
    type Item = io::Result<FileRecord>;

    fn next(&mut self) -> Option<Self::Item> {
        let Reverse(top) = self.heap.pop()?;
        let slot = self.readers.get_mut(top.run)?.as_mut()?;
        let head = slot.head.take()?;
        match read_framed(&mut slot.reader) {
            Ok(next) => {
                slot.head = next;
                if let Some(record) = slot.head.as_ref() {
                    self.heap
                        .push(Reverse(key_for(self.order, record, top.run)));
                }
            }
            Err(error) => return Some(Err(error)),
        }
        Some(Ok(head))
    }
}

#[cfg(test)]
#[path = "compact_merge/tests.rs"]
mod tests;
