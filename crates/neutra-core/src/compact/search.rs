//! Cancellable compact searches with bounded decoded blocks.

use super::*;
mod group;
mod scan;

impl CompactIndex {
    pub fn search(&self, q: &Query) -> io::Result<(Vec<SearchHit>, SearchStats)> {
        self.search_overlay(q, None, None, &|| false)
    }

    /// Search the immutable base and mutable WAL overlay as one logical index.
    /// Shadowed base paths are suppressed before ranking, so matched counts and
    /// result limits remain exact.
    pub fn search_with_delta(
        &self,
        q: &Query,
        delta: &DeltaIndex,
    ) -> io::Result<(Vec<SearchHit>, SearchStats)> {
        self.search_overlay(q, Some(delta), None, &|| false)
    }

    pub fn search_interruptible(
        &self,
        q: &Query,
        delta: Option<&DeltaIndex>,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> io::Result<(Vec<SearchHit>, SearchStats)> {
        self.search_overlay(q, delta, None, cancelled)
    }

    pub fn search_after_interruptible(
        &self,
        q: &Query,
        delta: Option<&DeltaIndex>,
        after: Option<&FileRecord>,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> io::Result<(Vec<SearchHit>, SearchStats)> {
        self.search_overlay(q, delta, after, cancelled)
    }

    fn search_overlay(
        &self,
        q: &Query,
        delta: Option<&DeltaIndex>,
        after: Option<&FileRecord>,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> io::Result<(Vec<SearchHit>, SearchStats)> {
        scan::in_search_pool(|| self.search_overlay_pooled(q, delta, after, cancelled))
    }

    fn search_overlay_pooled(
        &self,
        q: &Query,
        delta: Option<&DeltaIndex>,
        after: Option<&FileRecord>,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> io::Result<(Vec<SearchHit>, SearchStats)> {
        let started = Instant::now();
        let matcher = q.matcher()?;
        let after = after.map(|record| (matcher.score(record).unwrap_or(0), record));
        let candidates = self.candidate_blocks(q)?;
        let cmp = |a: &(u32, FileRecord), b: &(u32, FileRecord)| {
            compare_records(q.sort, &(a.0, &a.1), &(b.0, &b.1))
        };
        let prune_at = q.limit.saturating_mul(2).max(q.limit.saturating_add(32));
        // Several contiguous groups per thread let work stealing balance
        // uneven blocks; each group keeps only a pruned top-N, so peak
        // memory is groups × limit plus one block per thread.
        let groups = rayon::current_num_threads().saturating_mul(4).max(1);
        let group_len = candidates.len().div_ceil(groups).max(1);
        type BlockHits = io::Result<(u64, Vec<(u32, FileRecord)>)>;
        let mut decoded: Vec<BlockHits> = Vec::new();
        candidates
            .par_chunks(group_len)
            .map(|blocks| group::collect(self, blocks, &matcher, delta, after, cancelled))
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
            let parts = overlay.scan_upserts(
                rayon::current_num_threads(),
                Vec::<(u32, FileRecord)>::new,
                |part: &mut Vec<(u32, FileRecord)>, view| {
                    if cancelled() {
                        return Err(io::Error::new(
                            io::ErrorKind::Interrupted,
                            "search superseded",
                        ));
                    }
                    if let Some(hit) = Self::view_hit(&matcher, view) {
                        part.push(hit);
                    }
                    Ok(())
                },
            )?;
            for part in parts {
                for (score, record) in part {
                    matched += 1;
                    if past_cursor(q.sort, score, &record, after) {
                        ranked.push((score, record));
                    }
                    if q.limit > 0 && ranked.len() >= prune_at {
                        retain_best(&mut ranked, q.limit, &cmp);
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
        // Keep decoded pages out of idle RSS even when a selective query
        // touches fewer blocks of a large index. The OS file cache remains.
        if self.record_count > 1_000_000 {
            self.release_mapping();
        }
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
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
        // Trigrams are built from the stored spelling, so an accent-folded
        // term ("naive") would skip blocks holding "naïve". Scan them all.
        if q.fold_accents {
            return Ok((0..self.blocks.count as u32).collect());
        }
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
    pub(in crate::compact) fn decode_posting(&self, d: DictEntry) -> io::Result<Vec<u32>> {
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

fn past_cursor(
    sort: crate::SortKey,
    score: u32,
    record: &FileRecord,
    after: Option<(u32, &FileRecord)>,
) -> bool {
    after.is_none_or(|after| compare_records(sort, &(score, record), &after).is_gt())
}

#[cfg(test)]
mod oracle_tests;
#[cfg(test)]
mod tests;
