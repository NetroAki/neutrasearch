use super::*;
use std::io::Read;

pub(crate) fn stage_path(path: &Path) -> PathBuf {
    crate::compact::suffix_path(path, ".compact")
}

/// Finish a ready publication, or discard an interrupted preparation, before
/// starting another build.
pub(super) fn recover_streamed_publication(path: &Path) -> io::Result<()> {
    let marker = crate::compact::suffix_path(path, ".compacting");
    let staged = crate::compact::suffix_path(path, ".compact");
    let preparing = crate::compact::suffix_path(path, ".rebuilding");
    if !marker.is_file() {
        if preparing.is_file() {
            remove_staged_generation(path, &staged)?;
            remove_if_exists(&preparing)?;
            sync_parent(&preparing)?;
        }
        return Ok(());
    }
    let mut bytes = [0u8; 8];
    let marker_file = File::open(&marker)?;
    let mut marker_file = marker_file;
    if marker_file.read_exact(&mut bytes).is_err() {
        return Err(invalid("invalid compact rebuild publication marker"));
    }
    let generation = u64::from_le_bytes(bytes);
    let delta = delta_path(path);
    let wal_matches = crate::DeltaIndex::open_snapshot(&delta, generation).is_ok();
    if !wal_matches {
        // WAL replacement is the publication point for readers. If it has not
        // happened, old base and old WAL are still the complete generation.
        remove_staged_generation(path, &staged)?;
        std::fs::remove_file(&marker)?;
        remove_if_exists(&preparing)?;
        sync_parent(&marker)?;
        return Ok(());
    }
    if !staged.is_file() && CompactIndex::generation_on_disk(path)? == generation {
        remove_if_exists(&marker)?;
        remove_if_exists(&preparing)?;
        return sync_parent(path);
    }
    let base = CompactIndex::open(&staged)?;
    if base.generation() != generation {
        return Err(invalid("staged compact rebuild generation mismatch"));
    }
    drop(base);
    crate::dir_summary::DirectorySummary::open_for_compact(&staged, generation)?;
    crate::TreeSummary::open_for_compact(&staged, generation)?;
    publish_streamed_generation(path, &staged, generation)
}

pub(super) fn publish_streamed_generation(
    path: &Path,
    staged: &Path,
    generation: u64,
) -> io::Result<()> {
    let marker = crate::compact::suffix_path(path, ".compacting");
    let staged_delta = crate::compact::suffix_path(path, ".delta.compact");
    let delta = delta_path(path);

    let mut marker_bytes = [0u8; 8];
    File::open(&marker)?.read_exact(&mut marker_bytes)?;
    if u64::from_le_bytes(marker_bytes) != generation {
        return Err(invalid("compact rebuild marker generation mismatch"));
    }
    // Recovery can run after the new WAL was installed and accepted writes.
    // Never reset that canonical WAL: its matching generation is the
    // publication point and may already contain live changes.
    if crate::DeltaIndex::open_snapshot(&delta, generation).is_err() {
        if crate::DeltaIndex::open_snapshot(&staged_delta, generation).is_err() {
            let staged_writer = crate::DeltaIndex::replace_empty(&staged_delta, generation)?;
            drop(staged_writer);
            sync_parent(&staged_delta)?;
        }
        replace_file(&staged_delta, &delta)?;
        sync_parent(&delta)?;
    }

    CompactIndex::publish(staged, path)?;
    clear_stale_marker(path)?;
    std::fs::remove_file(&marker)?;
    sync_parent(&marker)?;
    // The staged base is no longer needed once the canonical base and
    // generation-bound sidecars are all durable.
    remove_if_exists(&crate::native_cursor::seed_path(staged))?;
    remove_if_exists(staged)?;
    remove_if_exists(&crate::dir_summary::DirectorySummary::path_for(staged))?;
    remove_if_exists(&crate::TreeSummary::path_for(staged))?;
    remove_if_exists(&crate::compact::suffix_path(&staged_delta, ".lock"))?;
    remove_if_exists(&crate::compact::suffix_path(path, ".rebuilding"))?;
    sync_parent(path)
}

pub(super) fn write_marker(path: &Path, generation: u64) -> io::Result<()> {
    let mut writer = open_private(path)?;
    writer.write_all(&generation.to_le_bytes())?;
    writer.flush()?;
    writer.get_ref().sync_all()?;
    drop(writer);
    sync_parent(path)
}

fn remove_staged_generation(path: &Path, staged: &Path) -> io::Result<()> {
    remove_if_exists(&crate::native_cursor::seed_path(staged))?;
    remove_if_exists(staged)?;
    remove_if_exists(&temp_path(staged))?;
    remove_if_exists(&crate::dir_summary::DirectorySummary::path_for(staged))?;
    remove_if_exists(&crate::compact::suffix_path(
        &crate::dir_summary::DirectorySummary::path_for(staged),
        ".new",
    ))?;
    remove_if_exists(&crate::TreeSummary::path_for(staged))?;
    remove_if_exists(&temp_path(&crate::TreeSummary::path_for(staged)))?;
    let staged_delta = crate::compact::suffix_path(path, ".delta.compact");
    remove_if_exists(&staged_delta)?;
    remove_if_exists(&crate::compact::suffix_path(&staged_delta, ".lock"))?;
    remove_if_exists(&crate::compact::suffix_path(
        &crate::dir_summary::DirectorySummary::path_for(staged),
        ".lock",
    ))?;
    remove_if_exists(&crate::compact::suffix_path(
        &crate::TreeSummary::path_for(staged),
        ".lock",
    ))?;
    Ok(())
}

pub(super) fn delta_path(path: &Path) -> PathBuf {
    let mut delta = path.to_path_buf();
    delta.set_extension("delta");
    delta
}

fn remove_if_exists(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
