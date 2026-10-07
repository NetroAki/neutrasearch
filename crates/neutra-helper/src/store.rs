//! The durable compact-base + delta-WAL store: open with crash recovery,
//! bounded delta application, and journaled compaction. Split from `main`
//! so the protocol loop stays separate from on-disk state.

use anyhow::{Context, Result};
 use neutra_core::{
     CompactIndex, DeltaChange, DeltaIndex, DirectorySummary, DirectorySummaryEntry,
     DirectorySummaryOverlay, Query, SearchHit, SearchStats, SpillAccumulator,
     DELTA_HEADER_BYTES,
 };
use std::io::{Read, Write};

pub(crate) struct DurableStore {
    pub(crate) path: std::path::PathBuf,
    #[cfg(test)]
    pub(crate) base: Option<CompactIndex>,
    #[cfg(not(test))]
    base: Option<CompactIndex>,
    pub(crate) delta: DeltaIndex,
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
        Ok(Self {
            path,
            base: Some(base),
            delta,
        })
    }

    /// The base and delta as one readable pair, for in-process reconcilers.
    pub(crate) fn view(&self) -> Option<(&CompactIndex, &DeltaIndex)> {
        self.base.as_ref().map(|base| (base, &self.delta))
    }

    pub(crate) fn search(&self, query: &Query) -> Result<(Vec<SearchHit>, SearchStats)> {
        let base = self
            .base
            .as_ref()
            .context("compact base is unavailable after a failed replacement")?;
        Ok(base.search_with_delta(query, &self.delta)?)
    }

    /// One directory's live totals: the `.dirs` sidecar with the WAL overlay
    /// applied, so serve-mode clients see fresh totals between compactions.
    pub(crate) fn directory_summary(
        &self,
        source: u32,
        path: &str,
    ) -> Result<Option<DirectorySummaryEntry>> {
        let base = self
            .base
            .as_ref()
            .context("compact base is unavailable after a failed replacement")?;
        let summary = DirectorySummary::open_for_compact(&self.path, base.generation())
            .context("open directory summary sidecar")?;
        if self.delta.change_count() == 0 {
            return Ok(summary.get(source, path).cloned());
        }
        let overlay = DirectorySummaryOverlay::from_delta(summary, base, &self.delta)?;
        Ok(overlay.get(source, path)?)
    }

    pub(crate) fn apply(&mut self, changes: Vec<DeltaChange>) -> Result<(u32, u64, bool)> {
        let count = self.delta.apply_batch(changes)?;
        self.delta.sync()?;
        Ok((count, self.delta.wal_bytes(), self.delta.needs_compaction()))
    }

    pub(crate) fn apply_bounded(&mut self, changes: Vec<DeltaChange>) -> Result<ApplyResult> {
        let mut compacted = None;
        if self.delta.needs_compaction() {
            compacted = Some(self.compact()?);
        }
        let (changes, _, needs_compaction) = self.apply(changes)?;
        if needs_compaction {
            compacted = Some(self.compact()?);
        }
        Ok(ApplyResult {
            changes,
            wal_bytes: self.delta.wal_bytes(),
            compacted,
        })
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
         let staged = compaction_stage(&self.path);
         let marker = compaction_marker(&self.path);
         let mut spill =
             SpillAccumulator::begin(&staged).context("spill base for compaction")?;
         CompactIndex::spill_compacted_base(base, &self.delta, &mut spill)
             .context("spill merged base for compaction")?;
         let built = CompactIndex::rebuild_streamed(spill.finish()?, &staged)
             .context("build staged replacement compact base")?;
        write_compaction_marker(&marker, built.generation)?;
        self.delta
            .reset(built.generation)
            .context("reset delta for replacement base")?;
        // Windows does not permit replacing a file while our old mmap is live.
        drop(self.base.take());
        CompactIndex::publish(&staged, &self.path).context("publish replacement compact base")?;
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

pub(crate) fn open_durable_pair(
    base_path: &std::path::Path,
    delta_path: &std::path::Path,
    compact_at: Option<u64>,
) -> Result<(CompactIndex, DeltaIndex)> {
    let marker = compaction_marker(base_path);
    if !marker.is_file() {
        let base = CompactIndex::open(base_path)
            .with_context(|| format!("open compact index {}", base_path.display()))?;
        let staged_path = compaction_stage(base_path);
        let short_wal_with_stage = staged_path.is_file()
            && std::fs::metadata(delta_path)
                .is_ok_and(|metadata| metadata.len() < DELTA_HEADER_BYTES);
        let current_delta = if short_wal_with_stage {
            Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "short WAL beside an unmarked compaction stage",
            ))
        } else {
            open_delta_writer(delta_path, base.generation(), compact_at)
        };
        let pair = match current_delta {
            Ok(delta) => (base, delta),
            Err(error) if retry_delta_with_staged_generation(&error) && staged_path.is_file() => {
                let staged =
                    CompactIndex::open(&staged_path).context("open unmarked compaction stage")?;
                let generation = staged.generation();
                let delta = match open_delta_writer(delta_path, generation, compact_at) {
                    Ok(delta) => delta,
                    Err(error) if recoverable_delta_error(&error) => {
                        replace_empty_delta(delta_path, generation, compact_at)
                            .context("replace torn WAL matching unmarked compaction stage")?
                    }
                    Err(error) => {
                        return Err(error).context("open WAL matching unmarked compaction stage");
                    }
                };
                drop(staged);
                drop(base);
                CompactIndex::publish(&staged_path, base_path)
                    .context("recover marker-lost base publication")?;
                let base =
                    CompactIndex::open(base_path).context("open marker-lost recovered base")?;
                if base.generation() != generation {
                    anyhow::bail!("marker-lost recovered base generation changed unexpectedly");
                }
                (base, delta)
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("open delta index {}", delta_path.display()));
            }
        };
        for stale in [
            append_suffix(&staged_path, ".new"),
            DirectorySummary::path_for(&staged_path),
            staged_path,
            compaction_marker_temp(base_path),
        ] {
            let _ = std::fs::remove_file(stale);
        }
        return Ok(pair);
    }

    let expected_generation = read_compaction_marker(&marker)?;
    let staged_path = compaction_stage(base_path);
    let base = CompactIndex::open(base_path)
        .with_context(|| format!("open compact index {}", base_path.display()))?;
    if base.generation() == expected_generation {
        let delta = match open_delta_writer(delta_path, expected_generation, compact_at) {
            Ok(delta) => delta,
            Err(error) if recoverable_delta_error(&error) => {
                replace_empty_delta(delta_path, expected_generation, compact_at)
                    .context("replace torn WAL after completed compaction")?
            }
            Err(error) => return Err(error).context("open delta after completed compaction"),
        };
        if staged_path.is_file() {
            DirectorySummary::publish(&staged_path, base_path, expected_generation)?;
            let _ = std::fs::remove_file(&staged_path);
        } else {
            let _ = std::fs::remove_file(DirectorySummary::path_for(base_path));
        }
        remove_compaction_marker(&marker)?;
        return Ok((base, delta));
    }

    let staged = CompactIndex::open(&staged_path)
        .with_context(|| format!("open compaction stage {}", staged_path.display()))?;
    if staged.generation() != expected_generation {
        anyhow::bail!("compaction marker and staged base generations differ");
    }
    drop(staged);

    let wal_needs_direct_replacement = match std::fs::metadata(delta_path) {
        Ok(metadata) => metadata.len() < DELTA_HEADER_BYTES,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(error) => return Err(error).context("inspect WAL during compaction recovery"),
    };
    let mut delta = if wal_needs_direct_replacement {
        replace_empty_delta(delta_path, expected_generation, compact_at)
            .context("replace short compaction WAL")?
    } else {
        match open_delta_writer(delta_path, base.generation(), compact_at) {
            Ok(mut delta) => {
                delta
                    .reset(expected_generation)
                    .context("finish compaction WAL reset")?;
                delta
            }
            Err(error) if retry_delta_with_staged_generation(&error) => {
                match open_delta_writer(delta_path, expected_generation, compact_at) {
                    Ok(delta) => delta,
                    Err(error) if recoverable_delta_error(&error) => {
                        replace_empty_delta(delta_path, expected_generation, compact_at)
                            .context("replace torn compaction WAL")?
                    }
                    Err(error) => {
                        return Err(error).context("reopen already-reset compaction WAL");
                    }
                }
            }
            Err(error) => {
                return Err(error).context("acquire delta writer during compaction recovery");
            }
        }
    };
    delta.sync()?;
    drop(base);
    CompactIndex::publish(&staged_path, base_path).context("finish staged base publication")?;
    let base = CompactIndex::open(base_path).context("open recovered compact base")?;
    if base.generation() != expected_generation {
        anyhow::bail!("recovered compact base generation changed unexpectedly");
    }
    remove_compaction_marker(&marker)?;
    let _ = std::fs::remove_file(&staged_path);
    Ok((base, delta))
}

pub(crate) fn open_delta_writer(
    path: &std::path::Path,
    generation: u64,
    compact_at: Option<u64>,
) -> std::io::Result<DeltaIndex> {
    match compact_at {
        Some(threshold) => DeltaIndex::open_with_threshold(path, generation, threshold),
        None => DeltaIndex::open(path, generation),
    }
}

pub(crate) fn replace_empty_delta(
    path: &std::path::Path,
    generation: u64,
    compact_at: Option<u64>,
) -> std::io::Result<DeltaIndex> {
    match compact_at {
        Some(threshold) => DeltaIndex::replace_empty_with_threshold(path, generation, threshold),
        None => DeltaIndex::replace_empty(path, generation),
    }
}

pub(crate) fn retry_delta_with_staged_generation(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::InvalidData | std::io::ErrorKind::UnexpectedEof
    )
}

pub(crate) fn recoverable_delta_error(error: &std::io::Error) -> bool {
    // A compaction reset can crash while rewriting the fixed-size WAL header.
    // Only that demonstrably incomplete state is recoverable. Complete but
    // invalid headers, frames, generations, and checksums must fail closed.
    error.kind() == std::io::ErrorKind::UnexpectedEof
}

pub(crate) fn compaction_stage(base: &std::path::Path) -> std::path::PathBuf {
    append_suffix(base, ".compact")
}

pub(crate) fn compaction_marker(base: &std::path::Path) -> std::path::PathBuf {
    append_suffix(base, ".compacting")
}

pub(crate) fn compaction_marker_temp(base: &std::path::Path) -> std::path::PathBuf {
    append_suffix(base, ".compacting.new")
}

pub(crate) fn stale_marker(base: &std::path::Path) -> std::path::PathBuf {
    append_suffix(base, ".stale")
}

pub(crate) fn write_stale_marker(base: &std::path::Path, reason: &str) -> Result<()> {
    let marker = stale_marker(base);
    if marker.is_file() {
        return Ok(());
    }
    let temporary = append_suffix(&marker, ".new");
    let _ = std::fs::remove_file(&temporary);
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
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
    let mut file = options.open(&temporary)?;
    let reason = reason.as_bytes();
    file.write_all(&reason[..reason.len().min(4096)])?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temporary, &marker)?;
    sync_parent(&marker)
}

pub(crate) fn acquire_rebuild_lock(base: &std::path::Path) -> Result<(std::fs::File, std::path::PathBuf)> {
    let mut delta = base.to_path_buf();
    delta.set_extension("delta");
    let lock_path = append_suffix(&delta, ".lock");
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let lock = options
        .open(&lock_path)
        .with_context(|| format!("open rebuild lock {}", lock_path.display()))?;
    let metadata = lock.metadata()?;
    if !metadata.is_file() {
        anyhow::bail!(
            "rebuild lock is not a regular file: {}",
            lock_path.display()
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 || metadata.mode() & 0o077 != 0 {
            anyhow::bail!(
                "rebuild lock must be private and single-linked: {}",
                lock_path.display()
            );
        }
    }
    fs2::FileExt::try_lock_exclusive(&lock).with_context(|| {
        format!(
            "index is in use by a writer; stop the serving helper before rebuilding {}",
            base.display()
        )
    })?;
    Ok((lock, delta))
}

pub(crate) fn append_suffix(path: &std::path::Path, suffix: &str) -> std::path::PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    value.into()
}

pub(crate) fn write_compaction_marker(path: &std::path::Path, generation: u64) -> Result<()> {
    if path.exists() {
        anyhow::bail!("compaction marker already exists: {}", path.display());
    }
    let temporary = append_suffix(path, ".new");
    let _ = std::fs::remove_file(&temporary);
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .with_context(|| format!("create compaction marker {}", temporary.display()))?;
    file.write_all(&generation.to_le_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temporary, path).with_context(|| {
        format!(
            "publish compaction marker {} -> {}",
            temporary.display(),
            path.display()
        )
    })?;
    sync_parent(path)
}

pub(crate) fn read_compaction_marker(path: &std::path::Path) -> Result<u64> {
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("open compaction marker {}", path.display()))?;
    if file.metadata()?.len() != 8 {
        anyhow::bail!("invalid compaction marker length");
    }
    let mut generation = [0u8; 8];
    file.read_exact(&mut generation)?;
    let generation = u64::from_le_bytes(generation);
    if generation == 0 {
        anyhow::bail!("invalid zero compaction generation");
    }
    Ok(generation)
}

pub(crate) fn remove_compaction_marker(path: &std::path::Path) -> Result<()> {
    std::fs::remove_file(path)
        .with_context(|| format!("remove compaction marker {}", path.display()))?;
    let _ = std::fs::remove_file(append_suffix(path, ".new"));
    sync_parent(path)
}

#[cfg(unix)]
pub(crate) fn sync_parent(path: &std::path::Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn sync_parent(_path: &std::path::Path) -> Result<()> {
    Ok(())
}
