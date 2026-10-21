use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::fs::File;
use std::io::{self, BufRead, BufReader, BufWriter, Read, Seek, Write};
use std::path::Path;

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
    write_postings_from_pairs(sorted_pairs(pairs_path)?, file)
}

/// Spilled (gram, block) pairs in ascending order. Segment files stay in the
/// spill directory next to `pairs_path` and vanish with it.
pub(crate) fn sorted_pairs(
    pairs_path: &Path,
) -> io::Result<Box<dyn Iterator<Item = io::Result<u64>>>> {
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
            return Ok(Box::new(segment.into_iter().map(Ok)));
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
    Ok(Box::new(std::iter::from_fn(move || {
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
    })))
}

fn write_segment(path: &Path, segment: &[u64]) -> io::Result<()> {
    let file = crate::compact_spill::create_private_file(path)?;
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

pub(crate) fn write_postings_from_pairs(
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
        let delta = if encoded.is_empty() {
            block
        } else {
            block - previous
        };
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
