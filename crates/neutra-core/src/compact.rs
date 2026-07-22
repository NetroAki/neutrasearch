//! Compact read-optimized index: compressed path blocks plus trigram postings.
//!
//! The base is immutable and mmapped on Unix. Windows snapshots use owned
//! bytes because Windows forbids atomically replacing a file with live mapped
//! views; this keeps compaction compatible with persistent readers.
use crate::{DeltaIndex, FileRecord, Query, SearchHit, SearchStats, SortKey};
#[cfg(not(windows))]
use memmap2::Mmap;
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

const MAGIC: &[u8; 8] = b"NEUTIDX1";
const VERSION: u32 = 3;
const HEADER: u64 = 64;
const CHECKSUM_BYTES: usize = 4;
const BLOCK_RECORDS: usize = 32;
const DESC_SIZE: u64 = 16;
const DICT_SIZE: u64 = 16;

#[derive(Clone, Copy)]
struct BlockDesc {
    offset: u64,
    len: u32,
    count: u16,
}
#[derive(Clone, Copy)]
struct DictEntry {
    gram: u32,
    len: u32,
    offset: u64,
}

#[cfg(not(windows))]
type IndexBytes = Mmap;
#[cfg(windows)]
type IndexBytes = Vec<u8>;

pub struct CompactIndex {
    map: IndexBytes,
    generation: u64,
    record_count: u64,
    blocks: Vec<BlockDesc>,
    dict: Vec<DictEntry>,
}

impl CompactIndex {
    pub fn build(records: &[FileRecord], path: &Path) -> io::Result<BuildStats> {
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
        let mut order = (0..records.len() as u32).collect::<Vec<_>>();
        order.sort_unstable_by(|a, b| records[*a as usize].path.cmp(&records[*b as usize].path));
        let mut descs = Vec::with_capacity(block_count);
        let mut postings = HashMap::<u32, Vec<u32>>::new();
        let mut grams = HashSet::<u32>::new();
        for (block_id, ids) in order.chunks(BLOCK_RECORDS).enumerate() {
            grams.clear();
            let refs = ids
                .iter()
                .map(|id| &records[*id as usize])
                .collect::<Vec<_>>();
            for record in &refs {
                collect_trigrams(&record.path, &mut grams);
            }
            for gram in grams.iter().copied() {
                postings.entry(gram).or_default().push(block_id as u32);
            }
            let raw = bincode::serialize(&refs).map_err(binerr)?;
            let compressed = zstd::bulk::compress(&raw, 1)?;
            let offset = file.stream_position()?;
            file.write_all(&compressed)?;
            descs.push(BlockDesc {
                offset,
                len: u32::try_from(compressed.len())
                    .map_err(|_| invalid("compressed block too large"))?,
                count: ids.len() as u16,
            });
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

    /// Atomically publish a fully built sibling index at the destination.
    /// The caller must release destination mmaps first on platforms that do
    /// not permit replacing a mapped file.
    pub fn publish(staged: &Path, destination: &Path) -> io::Result<()> {
        let verified = Self::open(staged)?;
        drop(verified);
        let temporary = temp_path(destination);
        let mut source = File::open(staged)?;
        let mut copy = open_private(&temporary)?;
        std::io::copy(&mut source, &mut copy)?;
        copy.flush()?;
        copy.get_ref().sync_all()?;
        drop(copy);
        replace_file(&temporary, destination)?;
        sync_parent(destination)
    }

    pub fn open(path: &Path) -> io::Result<Self> {
        let stale = suffix_path(path, ".stale");
        if stale.is_file() {
            return Err(invalid(format!(
                "compact index is marked stale by {} and requires a full rebuild",
                stale.display()
            )));
        }
        #[cfg(not(windows))]
        let map = {
            let file = File::open(path)?;
            unsafe { Mmap::map(&file)? }
        };
        #[cfg(windows)]
        let map = std::fs::read(path)?;
        if map.len() < HEADER as usize + CHECKSUM_BYTES || &map[..8] != MAGIC {
            return Err(invalid("not a Neutrasearch compact index"));
        }
        let data_len = map.len() - CHECKSUM_BYTES;
        let data = &map[..data_len];
        let expected_checksum = u32::from_le_bytes(
            map[data_len..]
                .try_into()
                .map_err(|_| invalid("missing compact index checksum"))?,
        );
        if crc32fast::hash(data) != expected_checksum {
            return Err(invalid("compact index checksum mismatch"));
        }
        if u32_at(data, 8)? != VERSION {
            return Err(invalid("unsupported compact index version"));
        }
        if u32_at(data, 12)? != BLOCK_RECORDS as u32 {
            return Err(invalid("unsupported path block size"));
        }
        let record_count = u64_at(data, 16)?;
        let generation = u64_at(data, 56)?;
        let block_count = u32_at(data, 24)? as usize;
        let dict_count = u32_at(data, 28)? as usize;
        let desc_offset = u64_at(data, 32)? as usize;
        let dict_offset = u64_at(data, 48)? as usize;
        let mut blocks = Vec::with_capacity(block_count);
        for i in 0..block_count {
            let p = desc_offset + i * DESC_SIZE as usize;
            let d = BlockDesc {
                offset: u64_at(data, p)?,
                len: u32_at(data, p + 8)?,
                count: u16_at(data, p + 12)?,
            };
            checked(data, d.offset as usize, d.len as usize)?;
            blocks.push(d);
        }
        let mut dict = Vec::with_capacity(dict_count);
        for i in 0..dict_count {
            let p = dict_offset + i * DICT_SIZE as usize;
            let d = DictEntry {
                gram: u32_at(data, p)?,
                len: u32_at(data, p + 4)?,
                offset: u64_at(data, p + 8)?,
            };
            checked(data, d.offset as usize, d.len as usize)?;
            dict.push(d);
        }
        if !dict.windows(2).all(|w| w[0].gram < w[1].gram) {
            return Err(invalid("trigram dictionary is not sorted"));
        }
        Ok(Self {
            map,
            generation,
            record_count,
            blocks,
            dict,
        })
    }

    /// Open a coherent read-only base+delta pair, including either side of an
    /// in-progress journaled compaction. Readers prefer the current pair and
    /// use the staged base only when its generation matches the reset WAL.
    pub fn open_with_delta_snapshot(path: &Path) -> io::Result<(Self, Option<DeltaIndex>)> {
        let base = Self::open(path)?;
        let mut delta_path = path.to_path_buf();
        delta_path.set_extension("delta");
        if !delta_path.is_file() {
            return Ok((base, None));
        }
        match DeltaIndex::open_snapshot(&delta_path, base.generation()) {
            Ok(delta) => Ok((base, Some(delta))),
            Err(current_error) => {
                let marker = suffix_path(path, ".compacting");
                let staged_path = suffix_path(path, ".compact");
                if !marker.is_file() || !staged_path.is_file() {
                    return Err(current_error);
                }
                let staged = Self::open(&staged_path)?;
                match DeltaIndex::open_snapshot(&delta_path, staged.generation()) {
                    Ok(delta) => Ok((staged, Some(delta))),
                    Err(staged_error) => Err(invalid(format!(
                        "neither current nor staged base matches the delta: current={current_error}; staged={staged_error}"
                    ))),
                }
            }
        }
    }

    pub fn len(&self) -> u64 {
        self.record_count
    }
    pub fn is_empty(&self) -> bool {
        self.record_count == 0
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Read just enough of the on-disk header to detect atomic base replacement.
    /// A caller that observes a change must reopen normally, which performs the
    /// complete structural and whole-file checksum validation.
    pub fn generation_on_disk(path: &Path) -> io::Result<u64> {
        let stale = suffix_path(path, ".stale");
        if stale.is_file() {
            return Err(invalid(format!(
                "compact index is marked stale by {} and requires a full rebuild",
                stale.display()
            )));
        }
        let mut file = File::open(path)?;
        let mut header = [0u8; HEADER as usize];
        file.read_exact(&mut header)?;
        if &header[..8] != MAGIC || u32_at(&header, 8)? != VERSION {
            return Err(invalid("invalid compact index header"));
        }
        u64_at(&header, 56)
    }

    pub fn mapped_bytes(&self) -> usize {
        self.map.len()
    }

    /// Return every base record in path order. Used by compaction to rewrite
    /// the base from current logical content rather than a WAL snapshot.
    pub fn records(&self) -> io::Result<Vec<FileRecord>> {
        let mut out = Vec::with_capacity(self.record_count as usize);
        for id in 0..self.blocks.len() as u32 {
            out.extend(self.read_block(id)?);
        }
        Ok(out)
    }

    pub fn search(&self, q: &Query) -> io::Result<(Vec<SearchHit>, SearchStats)> {
        self.search_overlay(q, None)
    }

    /// Search the immutable base and mutable WAL overlay as one logical index.
    /// Shadowed base paths are suppressed before ranking, so matched counts and
    /// result limits remain exact.
    pub fn search_with_delta(
        &self,
        q: &Query,
        delta: &DeltaIndex,
    ) -> io::Result<(Vec<SearchHit>, SearchStats)> {
        self.search_overlay(q, Some(delta))
    }

    fn search_overlay(
        &self,
        q: &Query,
        delta: Option<&DeltaIndex>,
    ) -> io::Result<(Vec<SearchHit>, SearchStats)> {
        let started = Instant::now();
        let candidates = self.candidate_blocks(q)?;
        let mut ranked = Vec::<(u32, FileRecord)>::new();
        let cmp = |a: &(u32, FileRecord), b: &(u32, FileRecord)| match q.sort {
            SortKey::Relevance => {
                b.0.cmp(&a.0)
                    .then(b.1.mtime.cmp(&a.1.mtime))
                    .then(a.1.path.cmp(&b.1.path))
            }
            SortKey::NameAsc => {
                a.1.name()
                    .to_ascii_lowercase()
                    .cmp(&b.1.name().to_ascii_lowercase())
                    .then(a.1.path.cmp(&b.1.path))
            }
            SortKey::PathAsc => a.1.path.cmp(&b.1.path),
            SortKey::SizeDesc => b.1.size.cmp(&a.1.size).then(a.1.path.cmp(&b.1.path)),
            SortKey::MtimeDesc => b.1.mtime.cmp(&a.1.mtime).then(a.1.path.cmp(&b.1.path)),
        };
        let prune_at = q.limit.saturating_mul(2).max(q.limit.saturating_add(32));
        let mut matched = 0u64;
        for block in candidates {
            for record in self.read_block(block)? {
                if delta.is_some_and(|overlay| overlay.shadows(record.path.as_ref())) {
                    continue;
                }
                if q.passes_filters(&record) {
                    if let Some(score) = q.score(&record) {
                        matched += 1;
                        ranked.push((score, record));
                    }
                }
            }
            if q.limit > 0 && ranked.len() >= prune_at {
                retain_best(&mut ranked, q.limit, &cmp);
            }
        }
        if let Some(overlay) = delta {
            for record in overlay.upserts() {
                if q.passes_filters(record) {
                    if let Some(score) = q.score(record) {
                        matched += 1;
                        ranked.push((score, record.clone()));
                        if q.limit > 0 && ranked.len() >= prune_at {
                            retain_best(&mut ranked, q.limit, &cmp);
                        }
                    }
                }
            }
        }
        if q.limit > 0 {
            retain_best(&mut ranked, q.limit, &cmp);
        }
        ranked.sort_unstable_by(&cmp);
        let hits = ranked
            .into_iter()
            .map(|(score, record)| SearchHit { score, record })
            .collect();
        Ok((
            hits,
            SearchStats {
                scanned: self.record_count
                    + delta.map_or(0, |overlay| overlay.change_count() as u64),
                matched,
                wall_us: started.elapsed().as_micros() as u64,
            },
        ))
    }

    fn candidate_blocks(&self, q: &Query) -> io::Result<Vec<u32>> {
        let mut grams = HashSet::new();
        for term in &q.terms {
            collect_trigrams(term, &mut grams);
        }
        if grams.is_empty() {
            return Ok((0..self.blocks.len() as u32).collect());
        }
        let mut entries = Vec::with_capacity(grams.len());
        for gram in grams {
            let Ok(i) = self.dict.binary_search_by_key(&gram, |d| d.gram) else {
                return Ok(Vec::new());
            };
            entries.push(self.dict[i]);
        }
        entries.sort_unstable_by_key(|d| d.len);
        let mut candidates = self.decode_posting(entries[0])?;
        // Three rare lists normally reduce candidates enough; exact verification
        // preserves correctness even when the remaining required grams are skipped.
        for entry in entries.into_iter().skip(1).take(2) {
            let right = self.decode_posting(entry)?;
            candidates = intersect(&candidates, &right);
            if candidates.is_empty() {
                break;
            }
        }
        Ok(candidates)
    }
    fn decode_posting(&self, d: DictEntry) -> io::Result<Vec<u32>> {
        let bytes = checked(&self.map, d.offset as usize, d.len as usize)?;
        let mut out = Vec::new();
        let mut p = 0;
        let mut id = 0u32;
        while p < bytes.len() {
            let delta = get_varint(bytes, &mut p)?;
            id = id
                .checked_add(delta)
                .ok_or_else(|| invalid("posting delta overflow"))?;
            out.push(id);
        }
        Ok(out)
    }
    fn read_block(&self, id: u32) -> io::Result<Vec<FileRecord>> {
        let d = *self
            .blocks
            .get(id as usize)
            .ok_or_else(|| invalid("path block ID out of range"))?;
        let compressed = checked(&self.map, d.offset as usize, d.len as usize)?;
        let raw = zstd::stream::decode_all(compressed)?;
        let records: Vec<FileRecord> = bincode::deserialize(&raw).map_err(binerr)?;
        if records.len() != d.count as usize {
            return Err(invalid("path block record count mismatch"));
        }
        Ok(records)
    }
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

fn collect_trigrams(text: &str, out: &mut HashSet<u32>) {
    let folded = text.to_lowercase();
    for w in folded.as_bytes().windows(3) {
        out.insert((w[0] as u32) << 16 | (w[1] as u32) << 8 | w[2] as u32);
    }
}
fn intersect(a: &[u32], b: &[u32]) -> Vec<u32> {
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::with_capacity(a.len().min(b.len()));
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                out.push(a[i]);
                i += 1;
                j += 1;
            }
        }
    }
    out
}
fn put_varint(mut n: u32, out: &mut Vec<u8>) {
    while n >= 0x80 {
        out.push((n as u8) | 0x80);
        n >>= 7;
    }
    out.push(n as u8);
}
fn get_varint(bytes: &[u8], p: &mut usize) -> io::Result<u32> {
    let (mut n, mut shift) = (0u32, 0);
    loop {
        let b = *bytes
            .get(*p)
            .ok_or_else(|| invalid("truncated posting varint"))?;
        *p += 1;
        if shift >= 32 {
            return Err(invalid("posting varint overflow"));
        }
        n |= ((b & 0x7f) as u32) << shift;
        if b & 0x80 == 0 {
            return Ok(n);
        }
        shift += 7;
    }
}
fn retain_best<F>(ranked: &mut Vec<(u32, FileRecord)>, limit: usize, compare: &F)
where
    F: Fn(&(u32, FileRecord), &(u32, FileRecord)) -> std::cmp::Ordering,
{
    if ranked.len() > limit {
        ranked.select_nth_unstable_by(limit, compare);
        ranked.truncate(limit);
    }
}

fn checked(bytes: &[u8], offset: usize, len: usize) -> io::Result<&[u8]> {
    bytes
        .get(
            offset
                ..offset
                    .checked_add(len)
                    .ok_or_else(|| invalid("index offset overflow"))?,
        )
        .ok_or_else(|| invalid("index section out of bounds"))
}
fn u16_at(b: &[u8], p: usize) -> io::Result<u16> {
    Ok(u16::from_le_bytes(checked(b, p, 2)?.try_into().unwrap()))
}
fn u32_at(b: &[u8], p: usize) -> io::Result<u32> {
    Ok(u32::from_le_bytes(checked(b, p, 4)?.try_into().unwrap()))
}
fn u64_at(b: &[u8], p: usize) -> io::Result<u64> {
    Ok(u64::from_le_bytes(checked(b, p, 8)?.try_into().unwrap()))
}
fn write_u16(w: &mut impl Write, n: u16) -> io::Result<()> {
    w.write_all(&n.to_le_bytes())
}
fn write_u32(w: &mut impl Write, n: u32) -> io::Result<()> {
    w.write_all(&n.to_le_bytes())
}
fn write_u64(w: &mut impl Write, n: u64) -> io::Result<()> {
    w.write_all(&n.to_le_bytes())
}
fn binerr(e: impl std::fmt::Display) -> io::Error {
    invalid(format!("index codec: {e}"))
}
fn invalid(e: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e.into())
}
fn safe_absolute_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    let windows_absolute = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\')
        || path.starts_with("\\\\");
    let portable_absolute = path.starts_with('/') || windows_absolute;
    !path.contains('\0')
        && portable_absolute
        && !path
            .split(['/', '\\'])
            .any(|component| matches!(component, "." | ".."))
}

fn new_generation() -> u64 {
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

fn temp_path(path: &Path) -> PathBuf {
    suffix_path(path, ".new")
}
fn clear_stale_marker(path: &Path) -> io::Result<()> {
    let stale = suffix_path(path, ".stale");
    match std::fs::remove_file(stale) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
fn suffix_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    value.into()
}
fn open_private(path: &Path) -> io::Result<BufWriter<File>> {
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
#[cfg(not(windows))]
fn replace_file(temp: &Path, path: &Path) -> io::Result<()> {
    std::fs::rename(temp, path)
}
#[cfg(windows)]
fn replace_file(temp: &Path, path: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    #[link(name = "kernel32")]
    extern "system" {
        fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
    }
    let existing = temp
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let replacement = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        MoveFileExW(
            existing.as_ptr(),
            replacement.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
#[cfg(unix)]
fn sync_parent(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}
#[cfg(not(unix))]
fn sync_parent(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FileKind, FsKind};
    fn rec(path: &str, size: u64) -> FileRecord {
        FileRecord {
            path: path.into(),
            size,
            mtime: size as i64,
            mode: 0,
            kind: FileKind::File,
            fs: FsKind::Btrfs,
            native_id: size + 100,
            native_parent: size + 10,
            source: 0,
        }
    }
    #[test]
    fn compact_roundtrip_and_substring_search() {
        let records = vec![
            rec("/home/a/AlphaDocument.txt", 1),
            rec("/home/a/beta.rs", 2),
            rec("/opt/gamma/notes.txt", 3),
        ];
        let path = std::env::temp_dir().join(format!(
            "neutra-compact-{}-roundtrip.idx",
            std::process::id()
        ));
        let first = CompactIndex::build(&records, &path).unwrap();
        let stats = CompactIndex::build(&records, &path).unwrap();
        assert_eq!(stats.records, 3);
        assert_ne!(stats.generation, 0);
        assert_ne!(stats.generation, first.generation);
        let index = CompactIndex::open(&path).unwrap();
        assert_eq!(index.generation(), stats.generation);
        let (hits, s) = index.search(&Query::parse("document ext:txt")).unwrap();
        assert_eq!(s.matched, 1);
        assert_eq!(hits[0].record.path.as_ref(), "/home/a/AlphaDocument.txt");
        assert_eq!(hits[0].record.native_id, 101);
        assert_eq!(hits[0].record.native_parent, 11);
        let (hits, _) = index.search(&Query::parse("gamma")).unwrap();
        assert_eq!(hits[0].record.path.as_ref(), "/opt/gamma/notes.txt");
        drop(index);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn stale_marker_blocks_readers_until_full_rebuild() {
        let path =
            std::env::temp_dir().join(format!("neutra-compact-stale-{}.idx", std::process::id()));
        let stale = suffix_path(&path, ".stale");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&stale);
        let old = CompactIndex::build(&[rec("/old.txt", 1)], &path).unwrap();
        assert_eq!(
            CompactIndex::generation_on_disk(&path).unwrap(),
            old.generation
        );
        std::fs::write(&stale, b"watch overflow").unwrap();
        assert!(CompactIndex::open(&path).is_err());
        assert!(CompactIndex::generation_on_disk(&path).is_err());

        let replacement = CompactIndex::build(&[rec("/new.txt", 2)], &path).unwrap();
        assert!(!stale.exists());
        assert_eq!(
            CompactIndex::generation_on_disk(&path).unwrap(),
            replacement.generation
        );
        assert!(CompactIndex::open(&path).is_ok());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn checksum_rejects_corrupted_compact_data() {
        let path =
            std::env::temp_dir().join(format!("neutra-compact-corrupt-{}.idx", std::process::id()));
        let _ = std::fs::remove_file(&path);
        CompactIndex::build(&[rec("/safe.txt", 1)], &path).unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[HEADER as usize] ^= 0x40;
        std::fs::write(&path, bytes).unwrap();
        assert!(CompactIndex::open(&path).is_err());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn build_rejects_non_normalized_paths() {
        let path =
            std::env::temp_dir().join(format!("neutra-invalid-path-{}.idx", std::process::id()));
        let _ = std::fs::remove_file(&path);
        assert!(CompactIndex::build(&[rec("/allowed/../secret", 1)], &path).is_err());
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn build_refuses_preplanted_temporary_symlink() {
        use std::os::unix::fs::symlink;

        let stem = format!("neutra-symlink-{}", std::process::id());
        let base_path = std::env::temp_dir().join(format!("{stem}.idx"));
        let temporary = temp_path(&base_path);
        let victim = std::env::temp_dir().join(format!("{stem}.victim"));
        let _ = std::fs::remove_file(&base_path);
        let _ = std::fs::remove_file(&temporary);
        let _ = std::fs::remove_file(&victim);
        std::fs::write(&victim, b"do not truncate").unwrap();
        symlink(&victim, &temporary).unwrap();

        assert!(CompactIndex::build(&[rec("/safe.txt", 1)], &base_path).is_err());
        assert_eq!(std::fs::read(&victim).unwrap(), b"do not truncate");
        std::fs::remove_file(temporary).unwrap();
        std::fs::remove_file(victim).unwrap();
    }

    #[test]
    fn publishing_replacement_keeps_existing_mmap_readable() {
        let stem = format!("neutra-publish-{}", std::process::id());
        let base_path = std::env::temp_dir().join(format!("{stem}.idx"));
        let staged_path = std::env::temp_dir().join(format!("{stem}.staged"));
        let _ = std::fs::remove_file(&base_path);
        let _ = std::fs::remove_file(&staged_path);
        CompactIndex::build(&[rec("/old.txt", 1)], &base_path).unwrap();
        let old = CompactIndex::open(&base_path).unwrap();
        CompactIndex::build(&[rec("/new.txt", 2)], &staged_path).unwrap();
        let staged_reader = CompactIndex::open(&staged_path).unwrap();

        CompactIndex::publish(&staged_path, &base_path).unwrap();
        let new = CompactIndex::open(&base_path).unwrap();
        assert_eq!(
            old.search(&Query::parse("old")).unwrap().0[0]
                .record
                .path
                .as_ref(),
            "/old.txt"
        );
        assert_eq!(
            new.search(&Query::parse("new")).unwrap().0[0]
                .record
                .path
                .as_ref(),
            "/new.txt"
        );
        assert_eq!(
            staged_reader.search(&Query::parse("new")).unwrap().0[0]
                .record
                .path
                .as_ref(),
            "/new.txt"
        );
        drop(new);
        drop(old);
        drop(staged_reader);
        std::fs::remove_file(base_path).unwrap();
        std::fs::remove_file(staged_path).unwrap();
    }

    #[test]
    fn snapshot_reader_selects_coherent_side_of_compaction_marker() {
        let stem = format!("neutra-pair-{}", std::process::id());
        let base_path = std::env::temp_dir().join(format!("{stem}.idx"));
        let mut delta_path = base_path.clone();
        delta_path.set_extension("delta");
        let staged_path = suffix_path(&base_path, ".compact");
        let marker_path = suffix_path(&base_path, ".compacting");
        for path in [&base_path, &delta_path, &staged_path, &marker_path] {
            let _ = std::fs::remove_file(path);
        }
        CompactIndex::build(&[rec("/old.txt", 1)], &base_path).unwrap();
        let old_generation = CompactIndex::open(&base_path).unwrap().generation();
        let mut delta = DeltaIndex::open(&delta_path, old_generation).unwrap();
        let staged = CompactIndex::build(&[rec("/new.txt", 2)], &staged_path).unwrap();
        std::fs::write(&marker_path, staged.generation.to_le_bytes()).unwrap();

        let (current, current_delta) = CompactIndex::open_with_delta_snapshot(&base_path).unwrap();
        assert_eq!(current.generation(), old_generation);
        assert_eq!(current_delta.unwrap().generation(), old_generation);
        drop(current);

        delta.reset(staged.generation).unwrap();
        let (replacement, replacement_delta) =
            CompactIndex::open_with_delta_snapshot(&base_path).unwrap();
        assert_eq!(replacement.generation(), staged.generation);
        assert_eq!(replacement_delta.unwrap().generation(), staged.generation);
        drop(replacement);
        drop(delta);

        let mut lock_path = delta_path.as_os_str().to_os_string();
        lock_path.push(".lock");
        for path in [
            base_path,
            delta_path,
            staged_path,
            marker_path,
            lock_path.into(),
        ] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn delta_upserts_and_tombstones_shadow_the_base() {
        let stem = format!("neutra-overlay-{}", std::process::id());
        let base_path = std::env::temp_dir().join(format!("{stem}.idx"));
        let delta_path = std::env::temp_dir().join(format!("{stem}.delta"));
        let _ = std::fs::remove_file(&base_path);
        let _ = std::fs::remove_file(&delta_path);
        CompactIndex::build(&[rec("/a/alpha.txt", 1), rec("/b/beta.txt", 2)], &base_path).unwrap();
        let base = CompactIndex::open(&base_path).unwrap();
        let mut delta = DeltaIndex::open(&delta_path, base.generation()).unwrap();
        delta
            .apply(crate::DeltaChange::Remove("/a/alpha.txt".into()))
            .unwrap();
        delta
            .apply(crate::DeltaChange::Upsert(rec("/b/beta.txt", 20)))
            .unwrap();
        delta
            .apply(crate::DeltaChange::Upsert(rec("/c/gamma.txt", 3)))
            .unwrap();

        let (hits, stats) = base
            .search_with_delta(&Query::parse("ext:txt"), &delta)
            .unwrap();
        assert_eq!(stats.matched, 2);
        assert_eq!(hits.len(), 2);
        assert!(!hits
            .iter()
            .any(|hit| hit.record.path.as_ref() == "/a/alpha.txt"));
        assert!(hits
            .iter()
            .any(|hit| hit.record.path.as_ref() == "/b/beta.txt" && hit.record.size == 20));
        assert!(hits
            .iter()
            .any(|hit| hit.record.path.as_ref() == "/c/gamma.txt"));

        drop(delta);
        drop(base);
        std::fs::remove_file(base_path).unwrap();
        std::fs::remove_file(delta_path).unwrap();
    }
}
