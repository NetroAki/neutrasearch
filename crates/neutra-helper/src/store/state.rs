use super::*;

pub(crate) struct DurableStore {
    pub(crate) path: std::path::PathBuf,
    #[cfg(test)]
    pub(crate) base: Option<CompactIndex>,
    #[cfg(not(test))]
    pub(super) base: Option<CompactIndex>,
    pub(crate) delta: DeltaIndex,
    settle_retry_at: Option<std::time::Instant>,
    pub(crate) background_compaction: bool,
}

pub(crate) struct CompactionResult {
    pub records: u64,
    pub bytes: u64,
}

pub(crate) struct ApplyResult {
    pub changes: u32,
    pub wal_bytes: u64,
    pub compacted: Option<CompactionResult>,
}

impl DurableStore {
    #[cfg(target_os = "linux")]
    pub(crate) const LIVE_COMMIT_BATCH: usize = 256;

    pub(crate) fn open(path: &std::path::Path) -> Result<Self> {
        Self::open_inner(path, None)
    }

    #[cfg(test)]
    pub(crate) fn open_with_threshold(path: &std::path::Path, compact_at: u64) -> Result<Self> {
        Self::open_inner(path, Some(compact_at))
    }

    fn open_inner(path: &std::path::Path, compact_at: Option<u64>) -> Result<Self> {
        let path = path.to_path_buf();
        let mut delta_path = path.clone();
        delta_path.set_extension("delta");
        let (base, delta) = open_durable_pair(&path, &delta_path, compact_at)?;
        // The catalog is part of the index: build it here so search works from
        // the first launch and after a format upgrade, without the GUI.
        if let Err(error) = neutra_core::BrowserIndex::ensure(&path, delta.generation()) {
            eprintln!("Cannot prepare the browse catalog: {error}");
        }
        let covered = neutra_core::BrowserIndex::open(&path, delta.generation())
            .ok()
            .map(|browser| browser.covers_delta(&path))
            .transpose()?
            .unwrap_or(false);
        if !covered {
            neutra_core::BrowserIndex::apply_delta(&path, &delta)
                .context("recover durable changes into browse orders")?;
        }
        neutra_core::BrowserIndex::mark_delta(&path, &delta)
            .context("publish recovered browse cursor")?;
        Ok(Self {
            path,
            base: Some(base),
            delta,
            settle_retry_at: None,
            background_compaction: false,
        })
    }

    pub(crate) fn apply(&mut self, changes: Vec<DeltaChange>) -> Result<(u32, u64, bool)> {
        let count = self.delta.apply_batch(changes.iter().cloned())?;
        self.delta.sync()?;
        neutra_core::BrowserIndex::apply_changes(&self.path, self.delta.generation(), &changes)
            .context("commit live changes into browse orders; durable delta retained")?;
        neutra_core::BrowserIndex::mark_delta(&self.path, &self.delta)
            .context("publish live browse cursor")?;
        Ok((count, self.delta.wal_bytes(), self.delta.needs_compaction()))
    }

    pub(crate) fn apply_bounded(&mut self, changes: Vec<DeltaChange>) -> Result<ApplyResult> {
        if self.background_compaction {
            anyhow::ensure!(self.base.is_some(), "base publication requires recovery");
            let (changes, wal_bytes, _) = self.apply(changes)?;
            return Ok(ApplyResult {
                changes,
                wal_bytes,
                compacted: None,
            });
        }
        let mut compacted = self.settle_delta()?;
        let (changes, _, _) = self.apply(changes)?;
        if let Some(result) = self.settle_delta()? {
            compacted = Some(result);
        }
        Ok(ApplyResult {
            changes,
            wal_bytes: self.delta.wal_bytes(),
            compacted,
        })
    }

    fn settle_delta(&mut self) -> Result<Option<CompactionResult>> {
        if !self.delta.needs_compaction() {
            return Ok(None);
        }
        if neutra_core::BrowserIndex::is_building(&self.path) {
            return Ok(None);
        }
        if self
            .settle_retry_at
            .is_some_and(|at| at > std::time::Instant::now())
        {
            return Ok(None);
        }
        // A failed fold leaves base and delta untouched, so it must not stop
        // live updates. Retry later instead of on every batch.
        match self.compact() {
            Ok(result) => {
                self.settle_retry_at = None;
                Ok(Some(result))
            }
            Err(error) if self.base.is_some() => {
                tracing::error!(%error, "delta fold failed; retrying in ten minutes");
                self.settle_retry_at =
                    Some(std::time::Instant::now() + std::time::Duration::from_secs(600));
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    /// Merge base+delta into a replacement base and reset the WAL. The caller
    /// holds the store write lock, so searches wait until the pair is coherent.
    pub(crate) fn compact(&mut self) -> Result<CompactionResult> {
        let base = self
            .base
            .as_ref()
            .context("compact base is unavailable after a failed replacement")?;
        // Stream the merge through a spill: materializing it peaked past
        // 25 GiB on large hosts. The spill merge restores global order.
        self.delta.sync().context("sync delta before compaction")?;
        let previous_generation = base.generation();
        neutra_core::BrowserIndex::apply_delta(&self.path, &self.delta)
            .context("commit browse changes before compaction")?;
        let staged = compaction_stage(&self.path);
        let marker = compaction_marker(&self.path);
        let built = match CompactIndex::fold_delta(base, &self.delta, &staged) {
            Ok(built) => built,
            Err(error) => {
                if error.kind() != std::io::ErrorKind::Unsupported {
                    tracing::warn!(%error, "fold failed; rebuilding the base from a spill");
                }
                rebuild_from_spill(base, &self.delta, &staged)?
            }
        };
        write_compaction_marker(&marker, built.generation)?;
        self.delta
            .reset(built.generation)
            .context("reset delta for replacement base")?;
        // Windows does not permit replacing a file while our old mmap is live.
        drop(self.base.take());
        CompactIndex::publish(&staged, &self.path).context("publish replacement compact base")?;
        neutra_core::BrowserIndex::advance_generation(
            &self.path,
            previous_generation,
            built.generation,
        )
        .context("keep browse orders across compact publication")?;
        neutra_core::BrowserIndex::mark_delta(&self.path, &self.delta)
            .context("publish browse cursor for the emptied delta")?;
        let base = CompactIndex::open(&self.path).context("open replacement compact base")?;
        if base.generation() != built.generation {
            anyhow::bail!("published compact base generation changed unexpectedly");
        }
        self.base = Some(base);
        remove_compaction_marker(&marker)?;
        let _ = std::fs::remove_file(&staged);
        Ok(CompactionResult {
            records: built.records,
            bytes: built.bytes,
        })
    }
}

pub(super) fn rebuild_from_spill(
    base: &CompactIndex,
    delta: &DeltaIndex,
    staged: &std::path::Path,
) -> Result<neutra_core::CompactBuildStats> {
    let mut spill = SpillAccumulator::begin(staged).context("spill base for compaction")?;
    CompactIndex::spill_compacted_base(base, delta, &mut spill)
        .context("spill merged base for compaction")?;
    CompactIndex::rebuild_streamed(spill.finish()?, staged)
        .context("build staged replacement compact base")
}
