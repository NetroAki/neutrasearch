//! Allocation-light block scanning. A search touches up to millions of
//! blocks, so each worker reuses one decompression buffer and reads records
//! as borrowed views instead of building owned strings per record.

use super::*;
use crate::record_ref::RecordRef;

/// Largest decompressed block we trust from a frame header.
const MAX_BLOCK_BYTES: u64 = 64 * 1024 * 1024;

pub(super) struct BlockScratch {
    raw: Vec<u8>,
    zstd: zstd::bulk::Decompressor<'static>,
}

impl BlockScratch {
    pub(super) fn new() -> io::Result<Self> {
        Ok(Self {
            raw: Vec::with_capacity(16 * 1024),
            zstd: zstd::bulk::Decompressor::new()?,
        })
    }
}

impl CompactIndex {
    /// Match one borrowed record against the query, allocating an owned
    /// record only when it passes.
    pub(super) fn view_hit(
        matcher: &crate::matcher::QueryMatcher,
        view: &RecordRef<'_>,
    ) -> Option<(u32, FileRecord)> {
        let q = matcher.query();
        if q.terms.is_empty() && q.regex.is_none() {
            let record = view.to_record();
            return q
                .passes_filters(&record)
                .then(|| matcher.score(&record).map(|score| (score, record)))
                .flatten();
        }
        matcher.score_path(view.path).and_then(|score| {
            let record = view.to_record();
            q.passes_filters(&record).then_some((score, record))
        })
    }

    /// Decode block `id` and hand every record to `visit` as a borrowed view.
    pub(super) fn scan_block(
        &self,
        id: u32,
        scratch: &mut BlockScratch,
        visit: &mut dyn FnMut(&RecordRef<'_>),
    ) -> io::Result<()> {
        if self.record_version < 4 {
            for record in self.read_block(id)? {
                visit(&RecordRef::from_record(&record));
            }
            return Ok(());
        }
        let desc = self
            .block(id as usize)
            .ok_or_else(|| invalid("path block ID out of range"))?;
        let compressed = checked(&self.map, desc.offset as usize, desc.len as usize)?;
        scratch.raw.clear();
        match zstd::zstd_safe::get_frame_content_size(compressed) {
            Ok(Some(size)) if size <= MAX_BLOCK_BYTES => {
                scratch.raw.reserve(size as usize);
                scratch
                    .zstd
                    .decompress_to_buffer(compressed, &mut scratch.raw)?;
            }
            _ => scratch.raw = zstd::stream::decode_all(compressed)?,
        }
        let records: Vec<RecordRef<'_>> = bincode::deserialize(&scratch.raw).map_err(binerr)?;
        if records.len() != desc.count as usize {
            return Err(invalid("path block record count mismatch"));
        }
        for record in &records {
            visit(record);
        }
        Ok(())
    }

    /// Hand the whole mapping back to the kernel after a large scan. The
    /// pages are clean file cache and fault back in on demand.
    pub(super) fn release_mapping(&self) {
        #[cfg(unix)]
        // SAFETY: the range is this index's own immutable mapping.
        unsafe {
            libc::madvise(
                self.map.as_ptr() as *mut libc::c_void,
                self.map.len(),
                libc::MADV_DONTNEED,
            );
        }
    }
}

/// Searches are short bursts: they get their own pool sized to the machine
/// (two cores stay free for the desktop). The threads sleep between queries
/// and hold no data, so nothing runs or grows in the background.
pub(crate) fn in_search_pool<R: Send>(work: impl FnOnce() -> R + Send) -> R {
    static POOL: std::sync::OnceLock<Option<rayon::ThreadPool>> = std::sync::OnceLock::new();
    let pool = POOL.get_or_init(|| {
        let threads = std::env::var("NEUTRASEARCH_SEARCH_THREADS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or_else(|| {
                std::thread::available_parallelism()
                    .map_or(4, usize::from)
                    .saturating_sub(2)
                    .clamp(2, 24)
            });
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|index| format!("neutra-search-{index}"))
            .build()
            .ok()
    });
    match pool {
        Some(pool) => pool.install(work),
        None => work(),
    }
}
