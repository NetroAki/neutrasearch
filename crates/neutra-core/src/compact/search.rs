//! Cancellable compact searches with bounded decoded blocks.

use super::*;

impl CompactIndex {
    pub fn search(&self, q: &Query) -> io::Result<(Vec<SearchHit>, SearchStats)> {
        self.search_overlay(q, None, &|| false)
    }

    /// Search the immutable base and mutable WAL overlay as one logical index.
    /// Shadowed base paths are suppressed before ranking, so matched counts and
    /// result limits remain exact.
    pub fn search_with_delta(
        &self,
        q: &Query,
        delta: &DeltaIndex,
    ) -> io::Result<(Vec<SearchHit>, SearchStats)> {
        self.search_overlay(q, Some(delta), &|| false)
    }

    pub fn search_interruptible(
        &self,
        q: &Query,
        delta: Option<&DeltaIndex>,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> io::Result<(Vec<SearchHit>, SearchStats)> {
        self.search_overlay(q, delta, cancelled)
    }

    fn search_overlay(
        &self,
        q: &Query,
        delta: Option<&DeltaIndex>,
        cancelled: &(dyn Fn() -> bool + Sync),
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
        let groups = rayon::current_num_threads().clamp(1, 8);
        let group_len = candidates.len().div_ceil(groups).max(1);
        type BlockHits = io::Result<(u64, Vec<(u32, FileRecord)>)>;
        let mut decoded: Vec<BlockHits> = Vec::new();
        candidates
            .par_chunks(group_len)
            .map(|group| {
                let mut matched = 0u64;
                let mut ranked = Vec::<(u32, FileRecord)>::new();
                for &block in group {
                    if cancelled() {
                        return Err(io::Error::new(
                            io::ErrorKind::Interrupted,
                            "search superseded",
                        ));
                    }
                    let records = self.read_block(block)?;
                    self.release_block(block);
                    let shadows = match delta {
                        Some(overlay) => overlay.shadows_batch(
                            &records.iter().map(|r| r.path.as_ref()).collect::<Vec<_>>(),
                        )?,
                        None => vec![false; records.len()],
                    };
                    for (record, shadowed) in records.into_iter().zip(shadows) {
                        if shadowed {
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
            overlay.for_each_upsert(|record| {
                if q.passes_filters(&record) {
                    if let Some(score) = matcher.score(&record) {
                        matched += 1;
                        ranked.push((score, record));
                        if q.limit > 0 && ranked.len() >= prune_at {
                            retain_best(&mut ranked, q.limit, &cmp);
                        }
                    }
                }
                Ok(())
            })?;
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
                scanned: self.record_count + delta.map_or(Ok(0), DeltaIndex::change_count)? as u64,
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
            return Ok((0..self.blocks.count as u32).collect());
        }
        let mut entries = Vec::with_capacity(grams.len());
        for gram in grams {
            let Some(entry) = self.dict_find(gram) else {
                return Ok(Vec::new());
            };
            entries.push(entry);
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
}
