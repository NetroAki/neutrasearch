//! Compact-index construction, journal-and-publish replacement, and the
//! rebuild lock. Split from the read/search side (`compact.rs`) so format
//! reading and index building evolve independently.

#[cfg(test)]
use crate::compact::{
    binerr, collect_trigrams, compare_index_paths, invalid, put_varint, BlockDesc,
    DictEntry, BLOCK_RECORDS, DESC_SIZE, HEADER, MAGIC, VERSION,
};
use crate::compact::suffix_path;
use crate::CompactIndex;
use crate::dir_summary::DirectorySummary;
#[cfg(test)]
use crate::query::safe_absolute_path;
#[cfg(test)]
use crate::FileRecord;
#[cfg(test)]
use rayon::prelude::*;
#[cfg(test)]
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
#[cfg(test)]
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::time::Instant;

 impl CompactIndex {
 /// In-RAM builders below are test-only: every production path streams
 /// through spills, since materializing a base peaked past 25 GiB.
 #[cfg(test)]
 pub fn build(records: &[FileRecord], path: &Path) -> io::Result<BuildStats> {
         let order = compact_record_order(records)?;
         Self::build_ordered(records, path, &order)
     }

 #[cfg(test)]
 fn build_ordered(records: &[FileRecord], path: &Path, order: &[u32]) -> io::Result<BuildStats> {
        if records.len() > u32::MAX as usize {
            return Err(invalid("compact index supports at most u32::MAX records"));
        }
        if records
            .iter()
            .any(|record| !safe_absolute_path(&record.path))
        {
            return Err(invalid(
                "compact index records must use absolute normalized paths",
            ));
        }
        let started = Instant::now();
        let generation = new_generation();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp = temp_path(path);
        let mut file = open_private(&temp)?;
        let block_count = records.len().div_ceil(BLOCK_RECORDS);
        file.get_ref()
            .set_len(HEADER + block_count as u64 * DESC_SIZE)?;
        file.seek(SeekFrom::Start(HEADER + block_count as u64 * DESC_SIZE))?;
        let mut descs = Vec::with_capacity(block_count);
        let mut postings = HashMap::<u32, Vec<u32>>::new();
        // Encode and compress blocks in parallel, then stream them out in
        // block order so offsets and posting lists stay identical to the
        // sequential layout. This is compaction's CPU bottleneck on
        // multi-million-record bases.
        let encoded = order
            .par_chunks(BLOCK_RECORDS)
            .map(|ids| {
                let mut grams = Vec::new();
                {
                    let mut set = HashSet::new();
                    for id in ids {
                        collect_trigrams(&records[*id as usize].path, &mut set);
                    }
                    grams.extend(set);
                }
                let refs = ids.iter().map(|id| &records[*id as usize]).collect::<Vec<_>>();
                let raw = bincode::serialize(&refs).map_err(binerr)?;
                let compressed = zstd::bulk::compress(&raw, 1)?;
                grams.shrink_to_fit();
                Ok((compressed, grams, ids.len() as u16))
            })
            .collect::<io::Result<Vec<_>>>()?;
        for (block_id, (compressed, grams, count)) in encoded.into_iter().enumerate() {
            let offset = file.stream_position()?;
            file.write_all(&compressed)?;
            descs.push(BlockDesc {
                offset,
                len: u32::try_from(compressed.len())
                    .map_err(|_| invalid("compressed block too large"))?,
                count,
            });
            for gram in grams {
                postings.entry(gram).or_default().push(block_id as u32);
            }
        }
        let postings_offset = file.stream_position()?;
        let mut keys = postings.into_iter().collect::<Vec<_>>();
        keys.sort_unstable_by_key(|(gram, _)| *gram);
        let mut dict = Vec::with_capacity(keys.len());
        for (gram, ids) in keys {
            let offset = file.stream_position()?;
            let mut encoded = Vec::with_capacity(ids.len() * 2);
            let mut previous = 0u32;
            for (i, id) in ids.into_iter().enumerate() {
                let delta = if i == 0 { id } else { id - previous };
                put_varint(delta, &mut encoded);
                previous = id;
            }
            file.write_all(&encoded)?;
            dict.push(DictEntry {
                gram,
                len: u32::try_from(encoded.len()).map_err(|_| invalid("posting list too large"))?,
                offset,
            });
        }
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
        write_u64(&mut file, records.len() as u64)?;
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
        let bytes = std::fs::metadata(path)?.len();
        Ok(BuildStats {
            generation,
            records: records.len() as u64,
            blocks: descs.len() as u32,
            trigrams: dict.len() as u32,
            bytes,
            wall_ms: started.elapsed().as_millis() as u64,
        })
    }

 /// Build a compact base and its generation-bound directory-summary sidecar.
     #[cfg(test)]
     pub fn build_with_summary(records: &[FileRecord], path: &Path) -> io::Result<BuildStats> {
        let compact_order = compact_record_order(records)?;
        let summary_order = crate::dir_summary::summary_order(records)?;
        let built = Self::build_ordered(records, path, &compact_order)?;
        DirectorySummary::build_sidecar_ordered(records, &summary_order, path, built.generation)?;
        Ok(built)
    }

 /// Rebuild a base while holding its single-writer delta lock, then remove
     /// the obsolete generation-bound WAL before readers reopen the pair.
     #[cfg(test)]
     pub fn rebuild(records: &[FileRecord], path: &Path) -> io::Result<BuildStats> {
        crate::compact_spill::with_rebuild_lock(path, || Self::build_with_summary(records, path))
    }

/// Atomically publish a fully built sibling index at the destination.
    /// The caller must release destination mmaps first on platforms that do
    /// not permit replacing a mapped file.
    pub fn publish(staged: &Path, destination: &Path) -> io::Result<()> {
        let verified = Self::open(staged)?;
        let generation = verified.generation();
        drop(verified);
        let temporary = temp_path(destination);
        let mut source = File::open(staged)?;
        let mut copy = open_private(&temporary)?;
        std::io::copy(&mut source, &mut copy)?;
        copy.flush()?;
        copy.get_ref().sync_all()?;
        drop(copy);
        replace_file(&temporary, destination)?;
        sync_parent(destination)?;
        DirectorySummary::publish(staged, destination, generation)
    }
}

 #[cfg(test)]
 fn compact_record_order(records: &[FileRecord]) -> io::Result<Vec<u32>> {
    if records.len() > u32::MAX as usize {
        return Err(invalid("compact index supports at most u32::MAX records"));
    }
    let mut order = (0..records.len() as u32).collect::<Vec<_>>();
    order.par_sort_by(|left, right| {
        compare_index_paths(
            records[*left as usize].path.as_ref(),
            records[*right as usize].path.as_ref(),
        )
    });
    Ok(order)
}

pub(crate) fn new_generation() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
        ^ (u64::from(std::process::id()) << 32);
    let mut current = NEXT.load(Ordering::Relaxed);
    loop {
        let candidate = current.max(seed).wrapping_add(1).max(1);
        match NEXT.compare_exchange_weak(current, candidate, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return candidate,
            Err(actual) => current = actual,
        }
    }
}

pub(crate) fn temp_path(path: &Path) -> PathBuf {
    suffix_path(path, ".new")
}

pub(crate) fn clear_stale_marker(path: &Path) -> io::Result<()> {
    let stale = suffix_path(path, ".stale");
    match std::fs::remove_file(stale) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(not(windows))]
pub(crate) fn replace_file(temp: &Path, path: &Path) -> io::Result<()> {
    std::fs::rename(temp, path)
}

#[cfg(unix)]
pub(crate) fn sync_parent(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

pub(crate) fn open_private(path: &Path) -> io::Result<BufWriter<File>> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true).read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    Ok(BufWriter::with_capacity(1024 * 1024, options.open(path)?))
}

pub(crate) fn write_u16(w: &mut impl Write, n: u16) -> io::Result<()> {
    w.write_all(&n.to_le_bytes())
}

pub(crate) fn write_u32(w: &mut impl Write, n: u32) -> io::Result<()> {
    w.write_all(&n.to_le_bytes())
}

pub(crate) fn write_u64(w: &mut impl Write, n: u64) -> io::Result<()> {
    w.write_all(&n.to_le_bytes())
}

#[derive(Debug, Clone, Copy)]
pub struct BuildStats {
    pub generation: u64,
    pub records: u64,
    pub blocks: u32,
    pub trigrams: u32,
    pub bytes: u64,
    pub wall_ms: u64,
}
