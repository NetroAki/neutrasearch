//! Compact read-optimized index: compressed path blocks plus trigram postings.
//!
//! The base is immutable and mmapped on Unix. Windows snapshots use owned
//! bytes because Windows forbids atomically replacing a file with live mapped
//! views; this keeps compaction compatible with persistent readers.
use crate::matcher::compare_records;
 use crate::{DeltaIndex, FileRecord, Query, SearchHit, SearchStats};
 use crate::compact_spill::SpillAccumulator;
#[cfg(not(windows))]
use memmap2::Mmap;
use rayon::prelude::*;
 use std::cmp::Ordering;
 use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::Instant;

pub(crate) const MAGIC: &[u8; 8] = b"NEUTIDX1";
 pub(crate) const VERSION: u32 = 4;
 /// Newest readable record layout. Bases at this version store disk bytes;
 /// version 3 bases remain readable with disk falling back to size.
 pub(crate) const MIN_READABLE_VERSION: u32 = 3;
pub(crate) const HEADER: u64 = 64;
const CHECKSUM_BYTES: usize = 4;
pub(crate) const BLOCK_RECORDS: usize = 32;
pub(crate) const DESC_SIZE: u64 = 16;
const DICT_SIZE: u64 = 16;

#[derive(Clone, Copy)]
pub(crate) struct BlockDesc {
    pub(crate) offset: u64,
    pub(crate) len: u32,
    pub(crate) count: u16,
}
#[derive(Clone, Copy)]
pub(crate) struct DictEntry {
    pub(crate) gram: u32,
    pub(crate) len: u32,
    pub(crate) offset: u64,
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
     record_version: u32,
 }

 /// One file row in a directory listing. Sizes track both views: on-disk
 /// for the tree, apparent for search-compatible reporting.
 #[derive(Debug, Clone)]
 pub struct DirFile {
     pub name: Box<str>,
     pub size: u64,
     pub logical: u64,
     pub kind: crate::FileKind,
 }
 /// One direct subdirectory with its subtree totals.
 #[derive(Debug, Clone)]
 pub struct DirSubdir {
     pub name: Box<str>,
     pub size: u64,
     pub logical: u64,
     pub files: u64,
 }
 /// A single directory fetched on demand: file rows plus subdirectory
 /// totals, bounded to keep tree browsing under a fixed memory budget.
 #[derive(Debug, Clone, Default)]
 pub struct DirListing {
     pub files: Vec<DirFile>,
     pub subdirs: Vec<DirSubdir>,
     pub total_size: u64,
     pub total_logical: u64,
     pub total_count: u64,
     pub files_truncated: bool,
 }

 impl DirListing {
     fn absorb(
         &mut self,
         record: &FileRecord,
         prefix: &str,
         subdirs: &mut HashMap<Box<str>, (u64, u64, u64)>,
     ) {
         let relative = &record.path.as_ref()[prefix.len()..];
         if relative.is_empty() {
             return;
         }
         let contributes = matches!(
             record.kind,
             crate::FileKind::File | crate::FileKind::Symlink
         );
         let size = if contributes { record.disk_bytes() } else { 0 };
         let logical = if contributes { record.size } else { 0 };
         match relative.find('/') {
             None => {
                 if record.kind == crate::FileKind::Dir {
                     // Empty directories still show as folders.
                     subdirs.entry(relative.into()).or_insert((0, 0, 0));
                     return;
                 }
                 self.total_size = self.total_size.saturating_add(size);
                 self.total_logical = self.total_logical.saturating_add(logical);
                 if contributes {
                     self.total_count += 1;
                 }
                 self.files.push(DirFile {
                     name: relative.into(),
                     size,
                     logical,
                     kind: record.kind,
                 });
             }
             Some(slash) => {
                 let bucket = subdirs
                     .entry(relative[..slash].into())
                     .or_insert((0, 0, 0));
                 bucket.0 = bucket.0.saturating_add(size);
                 bucket.1 = bucket.1.saturating_add(logical);
                 if contributes {
                     bucket.2 += 1;
                 }
                 self.total_size = self.total_size.saturating_add(size);
                 self.total_logical = self.total_logical.saturating_add(logical);
                 if contributes {
                     self.total_count += 1;
                 }
             }
         }
     }

     fn finish(
         mut self,
         subdirs: HashMap<Box<str>, (u64, u64, u64)>,
         base: &CompactIndex,
         delta: Option<&DeltaIndex>,
         prefix: &str,
     ) -> io::Result<Self> {
         if let Some(delta) = delta {
             for path in delta.removed() {
                 if strip_prefix_folded(path, prefix).is_some() {
                     // Exact base sizes resolve through the index; unknown
                     // paths (already-compacted churn) subtract nothing.
                     for record in base.records_by_path(path)? {
                         self.remove_record(&record, prefix);
                     }
                 }
             }
             for record in delta.upserts() {
                 if strip_prefix_folded(record.path.as_ref(), prefix).is_some() {
                     // An upsert supersedes any base row for the same file.
                     let contributes = matches!(
                         record.kind,
                         crate::FileKind::File | crate::FileKind::Symlink
                     );
                     self.drop_file_row(record.path.as_ref(), prefix, contributes);
                     let mut buckets = HashMap::new();
                     self.absorb(record, prefix, &mut buckets);
                     for (name, (size, logical, files)) in buckets {
                         self.merge_subdir(name, size, logical, files);
                     }
                 }
             }
         }
         let mut subdirs: Vec<DirSubdir> = subdirs
             .into_iter()
             .map(|(name, (size, logical, files))| DirSubdir { name, size, logical, files })
             .collect();
         subdirs.sort_unstable_by_key(|child| std::cmp::Reverse(child.size));
         self.files.sort_unstable_by_key(|file| std::cmp::Reverse(file.size));
         if self.files.len() > CompactIndex::MAX_LISTED_FILES {
             self.files.truncate(CompactIndex::MAX_LISTED_FILES);
             self.files_truncated = true;
         }
         self.subdirs = subdirs;
         Ok(self)
     }

     /// Drop a direct file row, adjusting totals by its stored size.
     /// `contributes` mirrors the record kind that earned the row its count.
     fn drop_file_row(&mut self, path: &str, prefix: &str, contributes: bool) {
         let Some(relative) = strip_prefix_folded(path, prefix) else {
             return;
         };
         if relative.is_empty() || relative.contains('/') {
             return;
         }
         if let Some(index) = self
             .files
             .iter()
             .position(|file| folded_eq(file.name.as_ref(), relative))
         {
             let removed = self.files.remove(index);
             self.total_size = self.total_size.saturating_sub(removed.size);
             self.total_logical = self.total_logical.saturating_sub(removed.logical);
             if contributes {
                 self.total_count = self.total_count.saturating_sub(1);
             }
         }
     }

     fn remove_record(&mut self, record: &FileRecord, prefix: &str) {
         let Some(relative) = strip_prefix_folded(record.path.as_ref(), prefix) else {
             return;
         };
         if relative.is_empty() {
             return;
         }
         let contributes = matches!(
             record.kind,
             crate::FileKind::File | crate::FileKind::Symlink
         );
         let size = if contributes { record.disk_bytes() } else { 0 };
         let logical = if contributes { record.size } else { 0 };
         match relative.find('/') {
             None => {
                 if record.kind == crate::FileKind::Dir {
                     // Directory tombstones drop the bucket; file totals
                     // leave through their own tombstones, as before.
                     if let Some(index) = self
                         .subdirs
                         .iter()
                         .position(|child| folded_eq(child.name.as_ref(), relative))
                     {
                         self.subdirs.remove(index);
                     }
                     return;
                 }
                 self.drop_file_row(record.path.as_ref(), prefix, contributes);
             }
             Some(slash) => {
                 if !contributes {
                     return;
                 }
                 // One deep file is gone: totals always move, the bucket
                 // follows only when this listing holds it.
                 self.total_size = self.total_size.saturating_sub(size);
                 self.total_logical = self.total_logical.saturating_sub(logical);
                 self.total_count = self.total_count.saturating_sub(1);
                 let name = &relative[..slash];
                 if let Some(bucket) = self
                     .subdirs
                     .iter_mut()
                     .find(|child| folded_eq(child.name.as_ref(), name))
                 {
                     bucket.size = bucket.size.saturating_sub(size);
                     bucket.logical = bucket.logical.saturating_sub(logical);
                     bucket.files = bucket.files.saturating_sub(1);
                 }
             }
         }
     }

     fn merge_subdir(&mut self, name: Box<str>, size: u64, logical: u64, files: u64) {
         match self
             .subdirs
             .iter_mut()
             .find(|child| folded_eq(child.name.as_ref(), name.as_ref()))
         {
             Some(child) => {
                 child.size = child.size.saturating_add(size);
                 child.logical = child.logical.saturating_add(logical);
                 child.files += files;
             }
             None => self.subdirs.push(DirSubdir { name, size, logical, files }),
         }
     }
 }

 /// Join a directory with a relative child name from a listing.
 pub fn join_child_path(dir: &str, name: &str) -> String {
     let dir = dir.trim_end_matches('/');
     if dir.is_empty() {
         format!("/{name}")
     } else {
         format!("{dir}/{name}")
     }
 }

 /// Scan prefix for one directory: normalized form always ends in `/` so
 /// the directory record itself sorts before the prefix and is skipped.
 fn dir_prefix(dir: &str) -> String {
     if dir == "/" {
         return "/".into();
     }
     let trimmed = dir.trim_end_matches('/');
     let mut prefix = String::with_capacity(trimmed.len() + 1);
     prefix.push_str(trimmed);
     prefix.push('/');
     prefix
 }

 /// Folded prefix match using the same byte fold as index ordering, so the
 /// scan window agrees with the block sort order on every platform.
 fn starts_with_folded(path: &str, prefix: &str) -> bool {
     let (path, prefix) = (path.as_bytes(), prefix.as_bytes());
     path.len() >= prefix.len()
         && path[..prefix.len()]
             .iter()
             .zip(prefix.iter())
             .all(|(left, right)| fold_byte(*left) == fold_byte(*right))
 }

 fn strip_prefix_folded<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
     starts_with_folded(path, prefix).then(|| &path[prefix.len()..])
 }

 impl CompactIndex {





    pub fn open(path: &Path) -> io::Result<Self> {
        Self::open_impl(path, true)
    }

    /// Open without verifying the whole-payload checksum.
    ///
    /// Startup path for readers (GUI, MCP, query CLI): opening a multi-gigabyte
    /// index would otherwise hash the entire file before the first search.
    /// Structural corruption is still caught — headers, block descriptors, and
    /// the trigram dictionary are bounds- and order-validated here, and every
    /// block is checksum-failed by zstd/bincode at decode time. Full-payload
    /// verification remains in `open` and on every write path.
    pub fn open_fast(path: &Path) -> io::Result<Self> {
        Self::open_impl(path, false)
    }

    fn open_impl(path: &Path, verify_payload: bool) -> io::Result<Self> {
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
        if verify_payload {
            let expected_checksum = u32::from_le_bytes(
                map[data_len..]
                    .try_into()
                    .map_err(|_| invalid("missing compact index checksum"))?,
            );
            if crc32fast::hash(data) != expected_checksum {
                return Err(invalid("compact index checksum mismatch"));
            }
        }
        let record_version = u32_at(data, 8)?;
        if record_version != VERSION && record_version != MIN_READABLE_VERSION {
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
            record_version,
        })
    }

    /// Open a coherent read-only base+delta pair, including either side of an
    /// in-progress journaled compaction. Readers prefer the current pair and
    /// use the staged base only when its generation matches the reset WAL.
    pub fn open_with_delta_snapshot(path: &Path) -> io::Result<(Self, Option<DeltaIndex>)> {
        Self::open_with_delta_snapshot_impl(path, true)
    }

    /// `open_with_delta_snapshot` with the startup fast path: the base is
    /// opened without whole-payload checksum verification (see `open_fast`).
    pub fn open_with_delta_snapshot_fast(path: &Path) -> io::Result<(Self, Option<DeltaIndex>)> {
        Self::open_with_delta_snapshot_impl(path, false)
    }

    fn open_with_delta_snapshot_impl(
        path: &Path,
        verify_payload: bool,
    ) -> io::Result<(Self, Option<DeltaIndex>)> {
        let base = Self::open_impl(path, verify_payload)?;
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
                let staged = Self::open_impl(&staged_path, verify_payload)?;
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
        if &header[..8] != MAGIC {
            return Err(invalid("invalid compact index header"));
        }
        let record_version = u32_at(&header, 8)?;
        if record_version != VERSION && record_version != MIN_READABLE_VERSION {
            return Err(invalid("invalid compact index header"));
        }
        u64_at(&header, 56)
    }

    pub fn mapped_bytes(&self) -> usize {
        self.map.len()
    }

    /// Return every base record in path order. Used by compaction to rewrite
    /// the base from current logical content rather than a WAL snapshot.
     /// Stream a base+delta merge into a spill for compaction. Materializing
     /// the merge peaked past 25 GiB on large hosts; this holds one block
     /// plus the small delta maps while the spill merge restores order.
     pub fn spill_compacted_base(
         base: &CompactIndex,
         delta: &DeltaIndex,
         spill: &mut SpillAccumulator,
     ) -> io::Result<()> {
         const BATCH: usize = 10_000;
         let removed: HashSet<&str> = delta.removed().map(|path| path.as_ref()).collect();
         let upserts: HashMap<&str, &FileRecord> = delta
             .upserts()
             .map(|record| (record.path.as_ref(), record))
             .collect();
         let mut batch = Vec::with_capacity(BATCH);
         for id in 0..base.blocks.len() as u32 {
             for record in base.read_block(id)? {
                 let path = record.path.as_ref();
                 if removed.contains(path) || upserts.contains_key(path) {
                     continue;
                 }
                 batch.push(record);
                 if batch.len() >= BATCH {
                     spill.push_batch(std::mem::take(&mut batch))?;
                     batch.reserve(BATCH);
                 }
             }
             base.release_block(id);
         }
         for record in delta.upserts() {
             batch.push(record.clone());
             if batch.len() >= BATCH {
                 spill.push_batch(std::mem::take(&mut batch))?;
                 batch.reserve(BATCH);
             }
         }
         if !batch.is_empty() {
             spill.push_batch(batch)?;
         }
         Ok(())
     }

     pub fn records(&self) -> io::Result<Vec<FileRecord>> {
        let mut out = Vec::with_capacity(self.record_count as usize);
        for id in 0..self.blocks.len() as u32 {
            out.extend(self.read_block(id)?);
        }
        Ok(out)
    }

    /// Find a base record by its source/path without materializing the whole
    /// compact index. Blocks are path-sorted, so common normalized paths use a
    /// logarithmic candidate lookup; unusual separator/case variants use a
    /// correctness-first full scan.
    pub fn record_by_path(&self, source: u32, path: &str) -> io::Result<Option<FileRecord>> {
        if let Some(record) = self.record_by_path_ordered(Some(source), path)? {
            return Ok(Some(record));
        }
        Ok(self
            .records_by_path(path)?
            .into_iter()
            .find(|record| record.source == source))
    }

    pub fn record_by_path_any_source(&self, path: &str) -> io::Result<Option<FileRecord>> {
        Ok(self.records_by_path(path)?.into_iter().next())
    }

    /// Return every base record whose source-independent path matches the
    /// delta path. Delta tombstones are path-keyed, so all matching sources
    /// must be shadowed consistently.
    ///
    /// Blocks are path-sorted, so common normalized paths use a logarithmic
    /// candidate lookup and only the matching run is decoded. This runs once
    /// per WAL change when overlays are built — a full scan here would make
    /// overlay construction quadratic.
    pub fn records_by_path(&self, path: &str) -> io::Result<Vec<FileRecord>> {
        let mut low = 0usize;
        let mut high = self.blocks.len();
        while low < high {
            let middle = low + (high - low) / 2;
            let records = self.read_block(middle as u32)?;
            let beyond = records
                .last()
                .is_some_and(|record| compare_index_paths(record.path.as_ref(), path) == Ordering::Less);
            if beyond {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        let mut matches = Vec::new();
        for block_id in low..self.blocks.len() {
            for record in self.read_block(block_id as u32)? {
                match compare_index_paths(record.path.as_ref(), path) {
                    Ordering::Less => {}
                    Ordering::Equal => {
                        if equivalent_index_path(record.path.as_ref(), path) {
                            matches.push(record);
                        }
                    }
                    Ordering::Greater => return Ok(matches),
                }
            }
        }
        Ok(matches)
    }

    fn record_by_path_ordered(
        &self,
        source: Option<u32>,
        path: &str,
    ) -> io::Result<Option<FileRecord>> {
        let mut low = 0usize;
        let mut high = self.blocks.len();
        while low < high {
            let middle = low + (high - low) / 2;
            let records = self.read_block(middle as u32)?;
            if records.last().is_some_and(|record| {
                compare_index_paths(record.path.as_ref(), path) == std::cmp::Ordering::Less
            }) {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        for block_id in low..self.blocks.len() {
            let records = self.read_block(block_id as u32)?;
            for record in records {
                match compare_index_paths(record.path.as_ref(), path) {
                    std::cmp::Ordering::Less => {}
                    std::cmp::Ordering::Equal => {
                        if equivalent_index_path(record.path.as_ref(), path)
                            && source.is_none_or(|wanted| wanted == record.source)
                        {
                            return Ok(Some(record));
                        }
                    }
                    std::cmp::Ordering::Greater => return Ok(None),
                }
            }
        }
        Ok(None)
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
        let matcher = q.matcher()?;
        let candidates = self.candidate_blocks(q)?;
        let cmp = |a: &(u32, FileRecord), b: &(u32, FileRecord)| {
            compare_records(q.sort, &(a.0, &a.1), &(b.0, &b.1))
        };
        let prune_at = q.limit.saturating_mul(2).max(q.limit.saturating_add(32));
        // Split the candidate blocks into a fixed number of contiguous
        // groups (not thousands of tiny chunks): each group streams its
        // blocks keeping only a pruned top-N, so peak memory is groups ×
        // limit plus one in-flight block per thread. Per-chunk collect and
        // reduce trees both re-moved every record through O(depth) merges
        // and burned minutes on full-base searches.
        const GROUPS: usize = 64;
        let group_len = candidates.len().div_ceil(GROUPS).max(1);
        let mut decoded: Vec<io::Result<(u64, Vec<(u32, FileRecord)>)>> = Vec::new();
        candidates
            .par_chunks(group_len)
            .map(|group| {
                let mut matched = 0u64;
                let mut ranked = Vec::<(u32, FileRecord)>::new();
                for &block in group {
                    for record in self.read_block(block)? {
                        if delta.is_some_and(|overlay| overlay.shadows(record.path.as_ref())) {
                            continue;
                        }
                        if q.passes_filters(&record) {
                            if let Some(score) = matcher.score(&record) {
                                matched += 1;
                                ranked.push((score, record));
                            }
                        }
                    }
                    if q.limit > 0 && ranked.len() >= prune_at {
                        retain_best(&mut ranked, q.limit, &cmp);
                    }
                }
                if q.limit > 0 && ranked.len() > q.limit {
                    retain_best(&mut ranked, q.limit, &cmp);
                }
                Ok((matched, ranked))
            })
            .collect_into_vec(&mut decoded);
        let mut ranked = Vec::<(u32, FileRecord)>::new();
        let mut matched = 0u64;
        for part in decoded {
            let (count, mut top) = part?;
            matched += count;
            ranked.append(&mut top);
            if q.limit > 0 && ranked.len() >= prune_at {
                retain_best(&mut ranked, q.limit, &cmp);
            }
        }
        if let Some(overlay) = delta {
            for record in overlay.upserts() {
                if q.passes_filters(record) {
                    if let Some(score) = matcher.score(record) {
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
        // Full-base searches transiently allocate gigabytes of decoded
        // records; hand fully-free pages back so a long-lived GUI does not
        // pin them in allocator arenas forever. No-op when small.
        #[cfg(unix)]
        if self.record_count > 1_000_000 {
            unsafe {
                libc::malloc_trim(0);
            }
        }
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
     /// Cap on file rows per directory listing. Beyond it the largest files
     /// are kept and the listing flags truncation; without a bound one
     /// monster directory could pin tens of megabytes in the tree cache.
     pub const MAX_LISTED_FILES: usize = 200_000;

     /// Drop a decoded block's pages back to the kernel. Bulk scans touch
     /// gigabytes of read-only mapping; clean pages fault back in on demand,
     /// so holding them only inflates resident size toward OOM.
     fn release_block(&self, id: u32) {
         #[cfg(unix)]
         {
             let Some(desc) = self.blocks.get(id as usize) else {
                 return;
             };
             if desc.len == 0 {
                 return;
             }
             let addr = (self.map.as_ptr() as usize).saturating_add(desc.offset as usize);
             // SAFETY: descriptor ranges were bounds-checked at open, and the
             // mapping is read-only so evicting clean pages is always safe.
             unsafe {
                 libc::madvise(
                     addr as *mut libc::c_void,
                     desc.len as usize,
                     libc::MADV_DONTNEED,
                 );
             }
         }
         #[cfg(not(unix))]
         {
             let _ = id;
         }
     }
     /// List one directory's direct children with subtree totals, streaming
     /// path-ordered blocks instead of materializing the base. Cost scales
     /// with the subtree, not the index; mapped pages are released as the
     /// scan advances. WAL changes merge on top with exact sizes resolved
     /// against the base, so totals stay correct while a delta is live.
     pub fn list_directory(
         &self,
         dir: &str,
         source: Option<u32>,
         delta: Option<&DeltaIndex>,
     ) -> io::Result<DirListing> {
         let prefix = dir_prefix(dir);
         // First block whose last record could fall under the prefix.
         let mut low = 0usize;
         let mut high = self.blocks.len();
         while low < high {
             let middle = low + (high - low) / 2;
             let records = self.read_block(middle as u32)?;
             self.release_block(middle as u32);
             if records.last().is_some_and(|record| {
                 compare_index_paths(record.path.as_ref(), &prefix) == Ordering::Less
             }) {
                 low = middle + 1;
             } else {
                 high = middle;
             }
         }
         let mut listing = DirListing::default();
         let mut subdirs: HashMap<Box<str>, (u64, u64, u64)> = HashMap::new();
         // Native lanes expose aliases: an exact source/path pair contributes
         // once, matching summary builds. The previous record moves along
         // with no extra allocation.
         // Seed from the block before the window so a duplicate pair
         // straddling the boundary still collapses.
         let mut previous: Option<FileRecord> = if low > 0 {
             self.read_block(low as u32 - 1)?.pop()
         } else {
             None
         };
         if low > 0 {
             // Pages from the binary search above are dropped just the same.
             self.release_block(low as u32 - 1);
         }
         for block_id in low..self.blocks.len() {
             let mut past = false;
             for record in self.read_block(block_id as u32)? {
                 // Source scoping composes with alias collapsing: twins share
                 // source and path, so both sides of the filter agree.
                 if source.is_some_and(|wanted| record.source != wanted) {
                     continue;
                 }
                 let duplicate = previous.as_ref().is_some_and(|prior| {
                     prior.source == record.source
                         && folded_eq(prior.path.as_ref(), record.path.as_ref())
                 });
                 if !duplicate {
                     let path = record.path.as_ref();
                     if starts_with_folded(path, &prefix) {
                         listing.absorb(&record, &prefix, &mut subdirs);
                     } else if compare_index_paths(path, &prefix) == Ordering::Greater {
                         past = true;
                         break;
                     }
                 }
                 previous = Some(record);
             }
             self.release_block(block_id as u32);
             if past {
                 break;
             }
         }
         listing.finish(subdirs, self, delta, &prefix)
     }
      fn read_block(&self, id: u32) -> io::Result<Vec<FileRecord>> {
        let d = *self
            .blocks
            .get(id as usize)
            .ok_or_else(|| invalid("path block ID out of range"))?;
        let compressed = checked(&self.map, d.offset as usize, d.len as usize)?;
        let raw = zstd::stream::decode_all(compressed)?;
        let records: Vec<FileRecord> = if self.record_version == VERSION {
            FileRecord::decode(&raw)
        } else {
            FileRecord::decode_old(&raw)
        }
        .map_err(binerr)?;
        if records.len() != d.count as usize {
            return Err(invalid("path block record count mismatch"));
        }
        Ok(records)
    }
}

pub(crate) fn collect_trigrams(text: &str, out: &mut HashSet<u32>) {
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
pub(crate) fn put_varint(mut n: u32, out: &mut Vec<u8>) {
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
pub(crate) fn binerr(e: impl std::fmt::Display) -> io::Error {
    invalid(format!("index codec: {e}"))
}
pub(crate) fn invalid(e: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e.into())
}

 /// Byte fold shared by index ordering and prefix scans so both agree on
 /// every platform.
 fn fold_byte(byte: u8) -> u8 {
     let byte = if byte == b'\\' { b'/' } else { byte };
     #[cfg(any(target_os = "windows", target_os = "macos"))]
     {
         byte.to_ascii_lowercase()
     }
     #[cfg(not(any(target_os = "windows", target_os = "macos")))]
     {
         byte
     }
 }

 fn folded_eq(left: &str, right: &str) -> bool {
     left.len() == right.len()
         && left
             .bytes()
             .zip(right.bytes())
             .all(|(left, right)| fold_byte(left) == fold_byte(right))
 }

 pub(crate) fn compare_index_paths(left: &str, right: &str) -> std::cmp::Ordering {
     let left = left.as_bytes();
     let right = right.as_bytes();
     for (left, right) in left.iter().zip(right) {
         match fold_byte(*left).cmp(&fold_byte(*right)) {
            std::cmp::Ordering::Equal => {}
            other => return other,
        }
    }
    left.len().cmp(&right.len())
}

fn equivalent_index_path(left: &str, right: &str) -> bool {
    let normalize = |path: &str| path.replace('\\', "/");
    let left = normalize(left);
    let right = normalize(right);
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        left.to_lowercase() == right.to_lowercase()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        left == right
    }
}


pub(crate) fn suffix_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    value.into()
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
#[cfg(not(unix))]
fn sync_parent(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compact_build::temp_path;
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
            disk: 0,
        }
    }
    #[test]
    fn open_rejects_unknown_record_versions() {
        let path = std::env::temp_dir().join(format!(
            "neutra-compact-badver-{}.nsx",
            std::process::id()
        ));
        let mut bytes = vec![0u8; HEADER as usize + CHECKSUM_BYTES];
        bytes[..8].copy_from_slice(MAGIC);
        bytes[8..12].copy_from_slice(&99u32.to_le_bytes());
        std::fs::write(&path, &bytes).unwrap();
        let error = match CompactIndex::open_fast(&path) {
            Ok(_) => panic!("unknown record version opened without error"),
            Err(error) => error,
        };
        assert!(
            error.to_string().contains("unsupported compact index version"),
            "unexpected error: {error}"
        );
        let _ = std::fs::remove_file(&path);
    }

     fn dir(path: &str) -> FileRecord {
         FileRecord {
             path: path.into(),
             size: 0,
             mtime: 0,
             mode: 0,
             kind: FileKind::Dir,
             fs: FsKind::Btrfs,
             native_id: 0,
             native_parent: 0,
             source: 0,
             disk: 0,
         }
     }

     #[test]
     fn list_directory_returns_direct_children_with_totals() {
         let mut records = vec![
             rec("/home/a/top.txt", 10),
             rec("/home/a/top.txt", 10),
             rec("/home/a/sub/deep.txt", 20),
             rec("/home/a/sub/deeper/x.txt", 5),
             dir("/home/a/empty"),
             rec("/home/b/other.txt", 7),
         ];
         // Apparent and on-disk sizes diverge: totals track both views.
         records[0].disk = 4;
         let path = std::env::temp_dir().join(format!(
             "neutra-compact-{}-listing.idx",
             std::process::id()
         ));
         CompactIndex::build(&records, &path).unwrap();
         let index = CompactIndex::open_fast(&path).unwrap();
         let listing = index.list_directory("/home/a", None, None).unwrap();
         // Duplicate alias pair counts once; outside paths stay out.
         assert_eq!(listing.total_size, 29);
         assert_eq!(listing.total_logical, 35);
         assert_eq!(listing.total_count, 3);
         assert_eq!(listing.files.len(), 1);
         assert_eq!(listing.files[0].name.as_ref(), "top.txt");
         assert_eq!(listing.files[0].size, 4);
         assert_eq!(listing.files[0].logical, 10);
         assert_eq!(listing.subdirs.len(), 2);
         assert_eq!(listing.subdirs[0].name.as_ref(), "sub");
         assert_eq!(listing.subdirs[0].size, 25);
         assert_eq!(listing.subdirs[0].files, 2);
         assert_eq!(listing.subdirs[1].name.as_ref(), "empty");
         let missing = index.list_directory("/nope", None, None).unwrap();
         assert_eq!(missing.total_count, 0);
         assert!(missing.files.is_empty() && missing.subdirs.is_empty());
         drop(index);
         std::fs::remove_file(path).unwrap();
     }

     #[test]
     fn list_directory_merges_live_delta_exactly() {
         let records = vec![rec("/d/f.txt", 10), rec("/d/sub/g.txt", 20)];
         let path = std::env::temp_dir().join(format!(
             "neutra-compact-{}-listing-delta.idx",
             std::process::id()
         ));
         CompactIndex::build(&records, &path).unwrap();
         let index = CompactIndex::open_fast(&path).unwrap();
         let wal = std::env::temp_dir().join(format!(
             "neutra-compact-{}-listing-delta.wal",
             std::process::id()
         ));
         let _ = std::fs::remove_file(&wal);
         let mut delta = DeltaIndex::open(&wal, index.generation()).unwrap();
         delta
             .apply(crate::DeltaChange::Upsert(rec("/d/f.txt", 30)))
             .unwrap();
         delta
             .apply(crate::DeltaChange::Upsert(rec("/d/new.txt", 5)))
             .unwrap();
         delta
             .apply(crate::DeltaChange::Remove("/d/sub/g.txt".into()))
             .unwrap();
         delta.sync().unwrap();
         let listing = index.list_directory("/d", None, Some(&delta)).unwrap();
         assert_eq!(listing.total_size, 35);
         assert_eq!(listing.total_count, 2);
         assert_eq!(listing.files.len(), 2);
         assert_eq!(listing.files[0].name.as_ref(), "f.txt");
         assert_eq!(listing.files[0].size, 30);
         drop(index);
         drop(delta);
         std::fs::remove_file(path).unwrap();
         std::fs::remove_file(&wal).unwrap();
         let mut lock = wal.into_os_string();
         lock.push(".lock");
         let _ = std::fs::remove_file(lock);
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
    fn rebuild_resets_the_delta_and_rejects_an_active_writer() {
        let stem = format!("neutra-rebuild-{}", std::process::id());
        let base_path = std::env::temp_dir().join(format!("{stem}.idx"));
        let mut delta_path = base_path.clone();
        delta_path.set_extension("delta");
        let lock_path = suffix_path(&delta_path, ".lock");
        for path in [&base_path, &delta_path, &lock_path] {
            let _ = std::fs::remove_file(path);
        }
        CompactIndex::build(&[rec("/old.txt", 1)], &base_path).unwrap();
        let generation = CompactIndex::open(&base_path).unwrap().generation();
        let writer = DeltaIndex::open(&delta_path, generation).unwrap();
        let error = CompactIndex::rebuild(&[rec("/new.txt", 2)], &base_path).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        drop(writer);

        CompactIndex::rebuild(&[rec("/new.txt", 2)], &base_path).unwrap();
        assert!(!delta_path.exists());
        let (rebuilt, delta) = CompactIndex::open_with_delta_snapshot(&base_path).unwrap();
        assert!(delta.is_none());
        assert_eq!(rebuilt.records().unwrap()[0].path.as_ref(), "/new.txt");

        std::fs::remove_file(base_path).unwrap();
        std::fs::remove_file(lock_path).unwrap();
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
