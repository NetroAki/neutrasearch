//! Block-level fold of a delta into a compact base. Blocks the delta does not
//! touch are copied byte for byte and their trigram postings are renumbered
//! from the old index; only touched or new blocks are re-encoded. A fold
//! writes about as much as it changes instead of re-sorting the machine.

use super::*;
use crate::compact_build::{
    clear_stale_marker, new_generation, replace_file, sync_parent, temp_path, BuildStats,
};
use crate::compact_merge::{RunMerge, SpillOrder};
use crate::compact_spill::{with_rebuild_lock, SpillAccumulator};
use std::path::Path;

mod output;
mod postings;
mod sidecar;
mod walk;

impl CompactIndex {
    /// Write a replacement base holding `base` with `delta` applied, plus its
    /// directory-summary sidecar, next to `staged`. Returns `Unsupported`
    /// when the layout needs a full rebuild (older record version, or sources
    /// that interleave in path order).
    pub fn fold_delta(
        base: &CompactIndex,
        delta: &DeltaIndex,
        staged: &Path,
    ) -> io::Result<BuildStats> {
        // Other platforms fold case in path order, which the summary feed here
        // does not reproduce, so they keep the full rebuild.
        if !cfg!(target_os = "linux") || base.record_version != VERSION {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "fold needs a base written at the current record version",
            ));
        }
        with_rebuild_lock(staged, || fold(base, delta, staged))
    }
}

fn fold(base: &CompactIndex, delta: &DeltaIndex, staged: &Path) -> io::Result<BuildStats> {
    let started = Instant::now();
    let generation = new_generation();
    let mut spill = SpillAccumulator::begin(staged)?;
    let mut batch = Vec::with_capacity(10_000);
    delta.for_each_upsert(|record| {
        batch.push(record);
        if batch.len() >= 10_000 {
            spill.push_batch(std::mem::take(&mut batch))?;
        }
        Ok(())
    })?;
    spill.push_batch(batch)?;
    let runs = spill.finish()?;
    let upserts = RunMerge::open(&runs, SpillOrder::Compact)?;
    let temp = temp_path(staged);
    let mut out = output::Output::begin(&temp, runs.dir()?, staged, generation, base)?;
    walk::run(base, delta, upserts, &mut out)?;
    let built = out.finish(base, generation)?;
    replace_file(&temp, staged)?;
    clear_stale_marker(staged)?;
    sync_parent(staged)?;
    out_sidecar_ready(staged, generation)?;
    Ok(BuildStats {
        wall_ms: started.elapsed().as_millis() as u64,
        bytes: std::fs::metadata(staged)?.len(),
        ..built
    })
}

fn out_sidecar_ready(staged: &Path, generation: u64) -> io::Result<()> {
    crate::TreeSummary::ensure(staged, generation)
}
