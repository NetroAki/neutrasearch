//! Compact read-optimized index: compressed path blocks plus trigram postings.
//!
//! The base is immutable and mmapped. Clean mapped pages are reclaimable, so
//! opening a large index does not imply keeping it resident while idle.
use crate::{DeltaIndex, FileRecord, Query, SearchHit, SearchStats, SortKey};
use memmap2::Mmap;
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

const MAGIC: &[u8; 8] = b"NEUTIDX1";
const VERSION: u32 = 2;
const HEADER: u64 = 64;
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

pub struct CompactIndex {
    map: Mmap,
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
        file.get_ref().sync_all()?;
        drop(file);
        replace_file(&temp, path)?;
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

    pub fn open(path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        let map = unsafe { Mmap::map(&file)? };
        if map.len() < HEADER as usize || &map[..8] != MAGIC {
            return Err(invalid("not a Neutrasearch compact index"));
        }
        if u32_at(&map, 8)? != VERSION {
            return Err(invalid("unsupported compact index version"));
        }
        if u32_at(&map, 12)? != BLOCK_RECORDS as u32 {
            return Err(invalid("unsupported path block size"));
        }
        let record_count = u64_at(&map, 16)?;
        let generation = u64_at(&map, 56)?;
        let block_count = u32_at(&map, 24)? as usize;
        let dict_count = u32_at(&map, 28)? as usize;
        let desc_offset = u64_at(&map, 32)? as usize;
        let dict_offset = u64_at(&map, 48)? as usize;
        let mut blocks = Vec::with_capacity(block_count);
        for i in 0..block_count {
            let p = desc_offset + i * DESC_SIZE as usize;
            let d = BlockDesc {
                offset: u64_at(&map, p)?,
                len: u32_at(&map, p + 8)?,
                count: u16_at(&map, p + 12)?,
            };
            checked(&map, d.offset as usize, d.len as usize)?;
            blocks.push(d);
        }
        let mut dict = Vec::with_capacity(dict_count);
        for i in 0..dict_count {
            let p = dict_offset + i * DICT_SIZE as usize;
            let d = DictEntry {
                gram: u32_at(&map, p)?,
                len: u32_at(&map, p + 4)?,
                offset: u64_at(&map, p + 8)?,
            };
            checked(&map, d.offset as usize, d.len as usize)?;
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

    pub fn len(&self) -> u64 {
        self.record_count
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn mapped_bytes(&self) -> usize {
        self.map.len()
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
        }
        if let Some(overlay) = delta {
            for record in overlay.upserts() {
                if q.passes_filters(record) {
                    if let Some(score) = q.score(record) {
                        matched += 1;
                        ranked.push((score, record.clone()));
                    }
                }
            }
        }
        let cmp = |a: &(u32, FileRecord), b: &(u32, FileRecord)| match q.sort {
            SortKey::Relevance => b.0.cmp(&a.0).then(b.1.mtime.cmp(&a.1.mtime)),
            SortKey::NameAsc => {
                a.1.name()
                    .to_ascii_lowercase()
                    .cmp(&b.1.name().to_ascii_lowercase())
            }
            SortKey::PathAsc => a.1.path.cmp(&b.1.path),
            SortKey::SizeDesc => b.1.size.cmp(&a.1.size),
            SortKey::MtimeDesc => b.1.mtime.cmp(&a.1.mtime),
        };
        if q.limit > 0 && ranked.len() > q.limit {
            ranked.select_nth_unstable_by(q.limit, &cmp);
            ranked.truncate(q.limit);
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
    let mut p = path.as_os_str().to_os_string();
    p.push(".new");
    p.into()
}
fn open_private(path: &Path) -> io::Result<BufWriter<File>> {
    let mut o = OpenOptions::new();
    o.create(true).truncate(true).write(true).read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    Ok(BufWriter::with_capacity(1024 * 1024, o.open(path)?))
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
