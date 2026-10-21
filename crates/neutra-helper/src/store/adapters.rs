use super::*;

pub(crate) fn open_durable_pair(
    base_path: &std::path::Path,
    delta_path: &std::path::Path,
    compact_at: Option<u64>,
) -> Result<(CompactIndex, DeltaIndex)> {
    CompactIndex::recover_rebuild(base_path).context("recover interrupted full rebuild")?;
    #[cfg(target_os = "linux")]
    let tail = super::maintenance::intent::check(base_path, delta_path)?;
    let pair = open_pair(base_path, delta_path, compact_at)?;
    #[cfg(target_os = "linux")]
    if tail {
        super::maintenance::intent::finish(base_path, delta_path)?;
    }
    Ok(pair)
}

fn open_pair(
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
                let previous_generation = base.generation();
                drop(base);
                CompactIndex::publish(&staged_path, base_path)
                    .context("recover marker-lost base publication")?;
                neutra_core::BrowserIndex::advance_generation(
                    base_path,
                    previous_generation,
                    generation,
                )
                .context("advance browse generation after marker-lost publication")?;
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
        // Marker-lost publication above went through CompactIndex::publish,
        // which already publishes the generation-bound directory and tree
        // sidecars. Only remove staged leftovers here; the browse
        // generation was advanced on the publish branch itself.
        for stale in [
            append_suffix(&staged_path, ".new"),
            DirectorySummary::path_for(&staged_path),
            TreeSummary::path_for(&staged_path),
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
            TreeSummary::publish(&staged_path, base_path, expected_generation)?;
            let _ = std::fs::remove_file(&staged_path);
        }
        neutra_core::BrowserIndex::recover_generation(base_path, expected_generation)
            .context("finish browse generation after completed publication")?;
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
    let previous_generation = base.generation();
    drop(base);
    CompactIndex::publish(&staged_path, base_path).context("finish staged base publication")?;
    neutra_core::BrowserIndex::advance_generation(
        base_path,
        previous_generation,
        expected_generation,
    )
    .context("advance browse generation after recovered publication")?;
    let base = CompactIndex::open(base_path).context("open recovered compact base")?;
    if base.generation() != expected_generation {
        anyhow::bail!("recovered compact base generation changed unexpectedly");
    }
    remove_compaction_marker(&marker)?;
    let _ = std::fs::remove_file(&staged_path);
    Ok((base, delta))
}
