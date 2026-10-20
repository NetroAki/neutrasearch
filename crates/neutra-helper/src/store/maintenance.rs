use super::*;
use neutra_core::{CompactBuildStats, CompactionCursor};
pub(super) mod intent;
mod worker;

pub(crate) struct FrozenCompaction {
    base: CompactIndex,
    delta: DeltaIndex,
    path: std::path::PathBuf,
    cursor: CompactionCursor,
}

pub(crate) struct PreparedCompaction {
    path: std::path::PathBuf,
    previous: u64,
    cursor: CompactionCursor,
    built: CompactBuildStats,
}

impl DurableStore {
    pub(crate) fn freeze_compaction(&self) -> Result<Option<FrozenCompaction>> {
        if !self.delta.needs_compaction() || neutra_core::BrowserIndex::is_building(&self.path) {
            return Ok(None);
        }
        let base = CompactIndex::open_fast(&self.path)?;
        anyhow::ensure!(
            Some(base.generation()) == self.base.as_ref().map(CompactIndex::generation),
            "base changed before compaction snapshot"
        );
        let delta = DeltaIndex::open_search_snapshot(self.delta.path(), base.generation())?;
        let cursor = delta.compaction_cursor();
        Ok(Some(FrozenCompaction {
            base,
            delta,
            path: self.path.clone(),
            cursor,
        }))
    }

    pub(crate) fn publish_compaction(&mut self, prepared: PreparedCompaction) -> Result<()> {
        anyhow::ensure!(
            self.base.as_ref().map(CompactIndex::generation) == Some(prepared.previous),
            "base changed during background compaction"
        );
        let root = self
            .delta
            .path()
            .parent()
            .context("delta has no parent")?
            .join(".delta.overlay");
        let tail = root.join(format!(
            "fold-tail-{}-{}",
            std::process::id(),
            prepared.built.generation
        ));
        let staged = compaction_stage(&prepared.path);
        let generation = prepared.built.generation;
        let bytes = self
            .delta
            .stage_compaction_tail(prepared.cursor, generation, &tail)?;
        // Once the WAL changes generation, startup can recover the durable
        // stage even if the marker was never written. Never reset away its tail.
        if let Err(error) = intent::write(&self.path, prepared.previous, generation, bytes) {
            if intent::finish(&self.path, self.delta.path()).is_err() {
                drop(self.base.take());
            }
            let _ = std::fs::remove_file(&tail);
            return Err(error);
        }
        drop(self.base.take());
        self.delta
            .install_compaction_tail(&tail, generation, bytes)?;
        write_compaction_marker(&compaction_marker(&self.path), generation)?;
        CompactIndex::publish(&staged, &self.path)?;
        neutra_core::BrowserIndex::advance_generation(&self.path, prepared.previous, generation)?;
        neutra_core::BrowserIndex::mark_delta(&self.path, &self.delta)?;
        let base = CompactIndex::open_fast(&self.path)?;
        anyhow::ensure!(
            base.generation() == generation,
            "published base generation changed"
        );
        remove_compaction_marker(&compaction_marker(&self.path))?;
        intent::finish(&self.path, self.delta.path())?;
        self.base = Some(base);
        let _ = std::fs::remove_file(staged);
        tracing::info!(
            records = prepared.built.records,
            bytes = prepared.built.bytes,
            tail_bytes = bytes,
            "background index compaction published"
        );
        Ok(())
    }
}

impl FrozenCompaction {
    pub(crate) fn build(self) -> Result<PreparedCompaction> {
        let staged = compaction_stage(&self.path);
        clean_stage(&staged)?;
        let built = match CompactIndex::fold_delta(&self.base, &self.delta, &staged) {
            Ok(built) => built,
            Err(error) if error.kind() == std::io::ErrorKind::Unsupported => {
                super::state::rebuild_from_spill(&self.base, &self.delta, &staged)?
            }
            Err(error) => return Err(error.into()),
        };
        Ok(PreparedCompaction {
            path: self.path.clone(),
            previous: self.base.generation(),
            cursor: self.cursor,
            built,
        })
    }
}

fn clean_stage(staged: &std::path::Path) -> Result<()> {
    for path in [
        staged.to_path_buf(),
        append_suffix(staged, ".new"),
        DirectorySummary::path_for(staged),
        TreeSummary::path_for(staged),
    ] {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
