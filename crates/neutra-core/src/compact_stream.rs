//! Streaming full-machine index over spilled runs. The compact merge feeds
//! blocks in path order, pairs sort externally, and the summary merge feeds
//! a second pass for the sidecar. Layout matches the in-RAM writer section
//! for section; only block-level parallelism is traded away (background
//! jobs prefer bounded RAM over encode speed).

use crate::compact::{
    binerr, collect_trigrams, invalid, BlockDesc, BLOCK_RECORDS, DESC_SIZE, HEADER, MAGIC, VERSION,
};
use crate::compact_build::{
    clear_stale_marker, new_generation, open_private, replace_file, sync_parent, temp_path,
    write_u16, write_u32, write_u64, BuildStats,
};
use crate::compact_merge::{write_postings_sorted, RunMerge, SpillOrder};
use crate::compact_spill::{with_rebuild_lock, SpillRuns};
use crate::compact_summary::{begin_sidecar, SummaryFeed};
use crate::CompactIndex;
use std::collections::HashSet;
use std::fs::File;
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

impl CompactIndex {
    /// Rebuild from spilled runs with bounded RAM, then publish atomically.
    /// Callers that already hold every record in memory keep `rebuild`.
    pub fn rebuild_streamed(runs: SpillRuns, path: &Path) -> io::Result<BuildStats> {
        with_rebuild_lock(path, || build_streamed(&runs, path))
    }
}

fn build_streamed(runs: &SpillRuns, path: &Path) -> io::Result<BuildStats> {
    let started = Instant::now();
    let count = runs.len();
    let generation = new_generation();
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let temp = temp_path(path);
    let mut file = open_private(&temp)?;
    let block_count = count.div_ceil(BLOCK_RECORDS as u64);
    file.get_ref().set_len(HEADER + block_count * DESC_SIZE)?;
    file.seek(SeekFrom::Start(HEADER + block_count * DESC_SIZE))?;
    let mut descs = Vec::with_capacity(block_count as usize);
    let pairs_path: PathBuf = runs.dir()?.join("pairs.psp");
    let pairs_file = File::create(&pairs_path)?;
    let mut pairs = BufWriter::with_capacity(1 << 20, pairs_file);
    let merge = RunMerge::open(runs, SpillOrder::Compact)?;
    let mut block = Vec::with_capacity(BLOCK_RECORDS);
    let mut block_id = 0u32;
    for record in merge {
        block.push(record?);
        if block.len() == BLOCK_RECORDS {
            encode_block(&block, block_id, &mut file, &mut descs, &mut pairs)?;
            block.clear();
            block_id += 1;
        }
    }
    if !block.is_empty() {
        encode_block(&block, block_id, &mut file, &mut descs, &mut pairs)?;
    }
    pairs.flush()?;
    drop(pairs);
    let postings_offset = file.stream_position()?;
    let dict = write_postings_sorted(&pairs_path, &mut file)?;
    let dict_offset = file.stream_position()?;
    for entry in &dict {
        write_u32(&mut file, entry.gram)?;
        write_u32(&mut file, entry.len)?;
        write_u64(&mut file, entry.offset)?;
    }
    file.seek(SeekFrom::Start(HEADER))?;
    for desc in &descs {
        write_u64(&mut file, desc.offset)?;
        write_u32(&mut file, desc.len)?;
        write_u16(&mut file, desc.count)?;
        write_u16(&mut file, 0)?;
    }
    file.seek(SeekFrom::Start(0))?;
    file.write_all(MAGIC)?;
    write_u32(&mut file, VERSION)?;
    write_u32(&mut file, BLOCK_RECORDS as u32)?;
    write_u64(&mut file, count)?;
    write_u32(&mut file, descs.len() as u32)?;
    write_u32(&mut file, dict.len() as u32)?;
    write_u64(&mut file, HEADER)?;
    write_u64(&mut file, postings_offset)?;
    write_u64(&mut file, dict_offset)?;
    write_u64(&mut file, generation)?;
    file.flush()?;
    let mut file = file.into_inner().map_err(|error| error.into_error())?;
    file.sync_all()?;
    file.seek(SeekFrom::Start(0))?;
    let mut checksum = crc32fast::Hasher::new();
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        checksum.update(&buffer[..read]);
    }
    file.write_all(&checksum.finalize().to_le_bytes())?;
    file.sync_all()?;
    drop(file);
    replace_file(&temp, path)?;
    clear_stale_marker(path)?;
    sync_parent(path)?;
    build_sidecar(runs, path, generation)?;
    crate::TreeSummary::ensure(path, generation)?;
    let bytes = std::fs::metadata(path)?.len();
    Ok(BuildStats {
        generation,
        records: count,
        blocks: descs.len() as u32,
        trigrams: dict.len() as u32,
        bytes,
        wall_ms: started.elapsed().as_millis() as u64,
    })
}

/// Encode one block exactly like the in-RAM writer, publish it immediately,
/// and spill its (gram, block) pairs for the external postings sort.
fn encode_block(
    block: &[crate::FileRecord],
    block_id: u32,
    file: &mut BufWriter<File>,
    descs: &mut Vec<BlockDesc>,
    pairs: &mut impl Write,
) -> io::Result<()> {
    let mut grams = HashSet::new();
    for record in block {
        collect_trigrams(&record.path, &mut grams);
    }
    let refs = block.iter().collect::<Vec<_>>();
    let raw = bincode::serialize(&refs).map_err(binerr)?;
    let compressed = zstd::bulk::compress(&raw, 1)?;
    let offset = file.stream_position()?;
    file.write_all(&compressed)?;
    descs.push(BlockDesc {
        offset,
        len: u32::try_from(compressed.len()).map_err(|_| invalid("compressed block too large"))?,
        count: block.len() as u16,
    });
    for gram in grams {
        let pair = ((gram as u64) << 32) | block_id as u64;
        pairs.write_all(&pair.to_le_bytes())?;
    }
    Ok(())
}

/// Stream the summary-order merge through the incremental feed into a fresh
/// sidecar, mirroring `build_with_summary` (compact base first, sidecar
/// second).
fn build_sidecar(runs: &SpillRuns, path: &Path, generation: u64) -> io::Result<()> {
    let mut sink = begin_sidecar(path, generation)?;
    let mut feed = SummaryFeed::new();
    let merge = RunMerge::open(runs, SpillOrder::Summary)?;
    for record in merge {
        feed.push(&record?, &mut |entry| sink.push(entry))?;
    }
    feed.finish(&mut |entry| sink.push(entry))?;
    sink.finish()
}
