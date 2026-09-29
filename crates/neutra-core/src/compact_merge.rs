//! Sorted runs and k-way merges over spilled chunks. Each pass sorts every
//! chunk by the pass key, writes run files, and streams the merged order;
//! only one head record per run stays resident.

use crate::compact::compare_index_paths;
use crate::compact_spill::{read_framed, write_framed, SpillRuns};
use crate::dir_summary::compare_paths;
use crate::FileRecord;
use rayon::prelude::*;
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::fs::File;
use std::io::{self, BufRead, BufReader, BufWriter, Read, Seek, Write};
use std::path::Path;

/// Merge order for a spill pass.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpillOrder {
    /// Compact-base order: path only.
    Compact,
    /// Directory-summary order: source, then path.
    Summary,
}

/// Merge key with a deterministic run-index tiebreak. Ordered ascending so
/// `Reverse` turns the max-heap into a min-merge.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct MergeKey {
    key: MergeKeyInner,
    run: usize,
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum MergeKeyInner {
    Compact(Box<str>),
    Summary(u32, Box<str>),
}

fn key_for(order: SpillOrder, record: &FileRecord, run: usize) -> MergeKey {
    let key = match order {
        SpillOrder::Compact => MergeKeyInner::Compact(record.path.clone()),
        SpillOrder::Summary => MergeKeyInner::Summary(record.source, record.path.clone()),
    };
    MergeKey { key, run }
}

fn compare(order: SpillOrder, left: &FileRecord, right: &FileRecord) -> std::cmp::Ordering {
    match order {
        SpillOrder::Compact => {
            compare_index_paths(left.path.as_ref(), right.path.as_ref())
        }
        SpillOrder::Summary => left.source.cmp(&right.source).then_with(|| {
            compare_paths(left.path.as_ref(), right.path.as_ref())
        }),
    }
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
        let mut readers: Vec<Option<RunReader>> =
            (0..runs.chunks().len()).map(|_| None).collect();
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
                readers[index] = Some(RunReader { reader, head: Some(head) });
            }
        }
        Ok(Self { readers, heap, order })
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
    let file = File::create(path)?;
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
                    self.heap.push(Reverse(key_for(self.order, record, top.run)));
                }
            }
            Err(error) => return Some(Err(error)),
        }
        Some(Ok(head))
    }
}

/// Pairs per postings sort segment: 8 bytes each, small enough to sort
/// beside the block encoder.
#[cfg(not(test))]
const PAIR_CHUNK: usize = 8_000_000;
#[cfg(test)]
const PAIR_CHUNK: usize = 1_000;

/// Externally sort spilled (gram, block) pairs and write postings with the
/// same delta-varint layout as the in-RAM builder. Streams the merged
/// pairs straight into the writer; only one segment plus merge buffers is
/// resident.
pub(crate) fn write_postings_sorted(
    pairs_path: &Path,
    file: &mut BufWriter<File>,
) -> io::Result<Vec<crate::compact::DictEntry>> {
    let mut segments = Vec::new();
    let pairs_file = File::open(pairs_path)?;
    let mut reader = BufReader::with_capacity(1 << 20, pairs_file);
    loop {
        let mut segment = Vec::with_capacity(PAIR_CHUNK);
        while segment.len() < PAIR_CHUNK {
            let mut bytes = [0u8; 8];
            match reader.read_exact(&mut bytes) {
                Ok(()) => segment.push(u64::from_le_bytes(bytes)),
                Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => break,
                Err(error) => return Err(error),
            }
        }
        if segment.is_empty() {
            break;
        }
        segment.sort_unstable();
        if reader.fill_buf()?.is_empty() && segments.is_empty() {
            return write_postings_from_pairs(segment.into_iter().map(Ok), file);
        }
        let path = pairs_path.with_extension(format!("seg{}", segments.len()));
        write_segment(&path, &segment)?;
        segments.push(path);
    }
    let mut readers = Vec::with_capacity(segments.len());
    let mut heap = BinaryHeap::new();
    for (index, path) in segments.iter().enumerate() {
        let reader = BufReader::with_capacity(1 << 20, File::open(path)?);
        readers.push(reader);
        if let Some(pair) = read_pair(&mut readers[index])? {
            heap.push((Reverse(pair), index));
        }
    }
    let ordered = std::iter::from_fn(|| {
        let (Reverse(pair), index) = heap.pop()?;
        match read_pair(&mut readers[index]) {
            Ok(next) => {
                if let Some(pair) = next {
                    heap.push((Reverse(pair), index));
                }
                Some(Ok(pair))
            }
            Err(error) => Some(Err(error)),
        }
    });
    let dict = write_postings_from_pairs(ordered, file)?;
    for path in &segments {
        let _ = std::fs::remove_file(path);
    }
    Ok(dict)
}

fn write_segment(path: &Path, segment: &[u64]) -> io::Result<()> {
    let file = File::create(path)?;
    let mut writer = BufWriter::with_capacity(1 << 20, file);
    for pair in segment {
        writer.write_all(&pair.to_le_bytes())?;
    }
    writer.flush()
}

fn read_pair(reader: &mut BufReader<File>) -> io::Result<Option<u64>> {
    let mut bytes = [0u8; 8];
    match reader.read_exact(&mut bytes) {
        Ok(()) => Ok(Some(u64::from_le_bytes(bytes))),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(None),
        Err(error) => Err(error),
    }
}

fn write_postings_from_pairs(
    pairs: impl Iterator<Item = io::Result<u64>>,
    file: &mut BufWriter<File>,
) -> io::Result<Vec<crate::compact::DictEntry>> {
    use crate::compact::put_varint;
    let mut dict = Vec::new();
    let mut current_gram = None::<u32>;
    let mut encoded = Vec::new();
    let mut previous = 0u32;
    let mut offset = file.stream_position()?;
    for pair in pairs {
        let pair = pair?;
        let gram = (pair >> 32) as u32;
        let block = pair as u32;
        if current_gram != Some(gram) {
            if let Some(previous_gram) = current_gram {
                dict.push(finish_posting(file, previous_gram, &encoded, &mut offset)?);
            }
            current_gram = Some(gram);
            encoded.clear();
            previous = 0;
        }
        let delta = if encoded.is_empty() { block } else { block - previous };
        put_varint(delta, &mut encoded);
        previous = block;
    }
    if let Some(previous_gram) = current_gram {
        dict.push(finish_posting(file, previous_gram, &encoded, &mut offset)?);
    }
    Ok(dict)
}

fn finish_posting(
    file: &mut BufWriter<File>,
    gram: u32,
    encoded: &[u8],
    offset: &mut u64,
) -> io::Result<crate::compact::DictEntry> {
    use crate::compact::invalid;
    file.write_all(encoded)?;
    let entry = crate::compact::DictEntry {
        gram,
        len: u32::try_from(encoded.len()).map_err(|_| invalid("posting list too large"))?,
        offset: *offset,
    };
    *offset += encoded.len() as u64;
    Ok(entry)
}
