use super::*;
use std::io;

impl CompactIndex {
    pub(crate) fn require_current_order(&self) -> io::Result<()> {
        if !cfg!(windows) && self.record_version < 7 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "legacy Unix index needs a native metadata rebuild before ordered subtree or catalog operations"));
        }
        Ok(())
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
        let mut batch = Vec::with_capacity(BATCH);
        for id in 0..base.blocks.count as u32 {
            let records = base.read_block(id)?;
            let shadowed = delta
                .shadows_batch(&records.iter().map(|r| r.path.as_ref()).collect::<Vec<_>>())?;
            for (record, shadowed) in records.into_iter().zip(shadowed) {
                if shadowed {
                    continue;
                }
                batch.push(record);
                if batch.len() >= BATCH {
                    spill.push_batch(std::mem::take(&mut batch))?;
                    batch.reserve(BATCH);
                }
            }
        }
        delta.for_each_upsert(|record| {
            batch.push(record);
            if batch.len() >= BATCH {
                spill.push_batch(std::mem::take(&mut batch))?;
                batch.reserve(BATCH);
            }
            Ok(())
        })?;
        if !batch.is_empty() {
            spill.push_batch(batch)?;
        }
        Ok(())
    }

    pub fn records(&self) -> io::Result<Vec<FileRecord>> {
        let mut out = Vec::with_capacity(self.record_count as usize);
        for id in 0..self.blocks.count as u32 {
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
        if !cfg!(windows) && (self.record_version < 7 || super::domain::windows_syntax(path)) {
            let mut found = Vec::new();
            for block in 0..self.blocks.count {
                found.extend(
                    self.read_block(block as u32)?
                        .into_iter()
                        .filter(|record| equivalent_index_path(&record.path, path)),
                );
            }
            return Ok(found);
        }
        let mut low = 0usize;
        let mut high = self.blocks.count;
        while low < high {
            let middle = low + (high - low) / 2;
            let records = self.read_block(middle as u32)?;
            let beyond = records.last().is_some_and(|record| {
                compare_index_paths(record.path.as_ref(), path) == Ordering::Less
            });
            if beyond {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        let mut matches = Vec::new();
        for block_id in low..self.blocks.count {
            if append_matching_records(self.read_block(block_id as u32)?, path, &mut matches) {
                return Ok(matches);
            }
        }
        Ok(matches)
    }

    fn record_by_path_ordered(
        &self,
        source: Option<u32>,
        path: &str,
    ) -> io::Result<Option<FileRecord>> {
        if !cfg!(windows) && (self.record_version < 7 || super::domain::windows_syntax(path)) {
            return Ok(self
                .records_by_path(path)?
                .into_iter()
                .find(|record| source.is_none_or(|wanted| wanted == record.source)));
        }
        let mut low = 0usize;
        let mut high = self.blocks.count;
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
        for block_id in low..self.blocks.count {
            let records = self.read_block(block_id as u32)?;
            if let std::ops::ControlFlow::Break(found) = ordered_record(records, source, path) {
                return Ok(found);
            }
        }
        Ok(None)
    }

    /// Cap on file rows per directory listing. Beyond it the largest files
    /// are kept and the listing flags truncation; without a bound one
    /// monster directory could pin tens of megabytes in the tree cache.
    pub const MAX_LISTED_FILES: usize = 200_000;

    /// Drop a decoded block's pages back to the kernel. Bulk scans touch
    /// gigabytes of read-only mapping; clean pages fault back in on demand,
    /// so holding them only inflates resident size toward OOM.
    pub(crate) fn release_block(&self, id: u32) {
        #[cfg(unix)]
        {
            let Some(desc) = self.block(id as usize) else {
                return;
            };
            if desc.len == 0 {
                return;
            }
            let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(1) as usize;
            // Linux fault-around maps neighbouring pages again after a narrow
            // eviction. Release that window too, including the descriptor, so
            // a sequential scan cannot accumulate the entire index in RSS.
            let window = page.max(64 * 1024);
            for (offset, len) in [
                (desc.offset as usize, desc.len as usize),
                (
                    self.blocks.start + id as usize * DESC_SIZE as usize,
                    DESC_SIZE as usize,
                ),
            ] {
                let start = offset / window * window;
                let end = ((offset + len).div_ceil(window) * window).min(self.map.len());
                let addr = (self.map.as_ptr() as usize).saturating_add(start);
                // SAFETY: both ranges belong to this immutable mapping.
                unsafe {
                    libc::madvise(addr as *mut libc::c_void, end - start, libc::MADV_DONTNEED);
                }
            }
        }
        #[cfg(not(unix))]
        {
            let _ = id;
        }
    }
    /// Decode a block and release its mapped pages after reading compressed data.
    pub(crate) fn read_block(&self, id: u32) -> io::Result<Vec<FileRecord>> {
        let d = self
            .block(id as usize)
            .ok_or_else(|| invalid("path block ID out of range"))?;
        let compressed = checked(&self.map, d.offset as usize, d.len as usize)?;
        if self.record_version >= 6
            && (compressed.len() < 5
                || compressed[..4] != [0x28, 0xb5, 0x2f, 0xfd]
                || compressed[4] & 4 == 0)
        {
            return Err(invalid("path block is missing its content checksum"));
        }
        let raw = zstd::stream::decode_all(compressed)?;
        self.release_block(id);
        let records: Vec<FileRecord> = if self.record_version >= 4 {
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

fn append_matching_records(
    records: Vec<FileRecord>,
    path: &str,
    matches: &mut Vec<FileRecord>,
) -> bool {
    for record in records {
        match compare_index_paths(record.path.as_ref(), path) {
            Ordering::Equal if equivalent_index_path(record.path.as_ref(), path) => {
                matches.push(record)
            }
            Ordering::Greater => return true,
            _ => {}
        }
    }
    false
}

fn ordered_record(
    records: Vec<FileRecord>,
    source: Option<u32>,
    path: &str,
) -> std::ops::ControlFlow<Option<FileRecord>> {
    for record in records {
        match compare_index_paths(record.path.as_ref(), path) {
            Ordering::Equal
                if equivalent_index_path(record.path.as_ref(), path)
                    && source.is_none_or(|wanted| wanted == record.source) =>
            {
                return std::ops::ControlFlow::Break(Some(record));
            }
            Ordering::Greater => return std::ops::ControlFlow::Break(None),
            _ => {}
        }
    }
    std::ops::ControlFlow::Continue(())
}
