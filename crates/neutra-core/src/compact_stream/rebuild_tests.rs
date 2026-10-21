use super::publication::delta_path;
use super::*;

fn rec(path: &str, native_id: u64) -> crate::FileRecord {
    crate::FileRecord {
        path: path.into(),
        source: 0,
        fs: crate::FsKind::Btrfs,
        kind: crate::FileKind::File,
        size: native_id,
        mtime: 1,
        mode: 0,
        disk: 0,
        native_id,
        native_parent: native_id.saturating_sub(1),
    }
}

fn paths(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "neutra-stream-publish-{label}-{}.idx",
        std::process::id()
    ))
}

fn clean(path: &Path) {
    for candidate in [
        path.to_path_buf(),
        delta_path(path),
        crate::compact::suffix_path(path, ".compact"),
        crate::compact::suffix_path(path, ".compact.new"),
        crate::compact::suffix_path(path, ".compacting"),
        crate::compact::suffix_path(path, ".rebuilding"),
        crate::dir_summary::DirectorySummary::path_for(&crate::compact::suffix_path(
            path, ".compact",
        )),
        crate::TreeSummary::path_for(&crate::compact::suffix_path(path, ".compact")),
        crate::compact::suffix_path(path, ".delta.compact"),
        crate::compact::suffix_path(path, ".delta.compact.lock"),
    ] {
        let _ = std::fs::remove_file(candidate);
    }
}

fn runs(records: Vec<crate::FileRecord>, output: &Path) -> SpillRuns {
    let mut spill = crate::SpillAccumulator::begin(output).unwrap();
    spill.push_batch(records).unwrap();
    spill.finish().unwrap()
}

#[test]
fn sidecar_failure_keeps_old_base_and_wal_and_retry_recovers_preparation() {
    let path = paths("sidecar-retry");
    clean(&path);
    CompactIndex::build_with_summary(&[rec("/old.txt", 1)], &path).unwrap();
    let old_generation = CompactIndex::generation_on_disk(&path).unwrap();
    let wal = delta_path(&path);
    let mut writer = crate::DeltaIndex::open(&wal, old_generation).unwrap();
    writer
        .apply(crate::DeltaChange::Upsert(rec("/wal-only.txt", 2)))
        .unwrap();
    drop(writer);

    let staged = crate::compact::suffix_path(&path, ".compact");
    let summary = crate::dir_summary::DirectorySummary::path_for(&staged);
    std::fs::create_dir(&summary).unwrap();
    assert!(CompactIndex::rebuild_streamed(runs(vec![rec("/new.txt", 3)], &path), &path).is_err());
    assert_eq!(
        CompactIndex::generation_on_disk(&path).unwrap(),
        old_generation
    );
    let (base, delta) = CompactIndex::open_with_delta_snapshot(&path).unwrap();
    assert_eq!(base.generation(), old_generation);
    assert_eq!(delta.unwrap().generation(), old_generation);
    drop(base);

    std::fs::remove_dir(summary).unwrap();
    assert!(CompactIndex::rebuild_streamed(runs(vec![rec("/new.txt", 3)], &path), &path).is_ok());
    let (base, delta) = CompactIndex::open_with_delta_snapshot(&path).unwrap();
    assert!(delta.is_some());
    assert_eq!(base.records().unwrap()[0].path.as_ref(), "/new.txt");
    drop(base);
    clean(&path);
}

#[test]
fn ready_marker_with_old_wal_discards_stage_and_preserves_old_pair() {
    let path = paths("old-wal");
    clean(&path);
    CompactIndex::build_with_summary(&[rec("/old.txt", 1)], &path).unwrap();
    let old_generation = CompactIndex::generation_on_disk(&path).unwrap();
    crate::DeltaIndex::open(&delta_path(&path), old_generation).unwrap();
    let staged = crate::compact::suffix_path(&path, ".compact");
    let stats = CompactIndex::build_with_summary(&[rec("/new.txt", 2)], &staged).unwrap();
    write_marker(
        &crate::compact::suffix_path(&path, ".compacting"),
        stats.generation,
    )
    .unwrap();
    write_marker(
        &crate::compact::suffix_path(&path, ".rebuilding"),
        stats.generation,
    )
    .unwrap();
    recover_streamed_publication(&path).unwrap();
    assert_eq!(
        CompactIndex::generation_on_disk(&path).unwrap(),
        old_generation
    );
    let (base, delta) = CompactIndex::open_with_delta_snapshot(&path).unwrap();
    assert_eq!(base.generation(), old_generation);
    assert_eq!(delta.unwrap().generation(), old_generation);
    drop(base);
    clean(&path);
}

#[test]
fn recovery_retries_after_wal_install_and_publishes_matching_generation() {
    let path = paths("retry-publish");
    clean(&path);
    CompactIndex::build_with_summary(&[rec("/old.txt", 1)], &path).unwrap();
    let old_generation = CompactIndex::generation_on_disk(&path).unwrap();
    crate::DeltaIndex::open(&delta_path(&path), old_generation).unwrap();
    let staged = crate::compact::suffix_path(&path, ".compact");
    let stats = CompactIndex::build_with_summary(&[rec("/new.txt", 2)], &staged).unwrap();
    write_marker(
        &crate::compact::suffix_path(&path, ".compacting"),
        stats.generation,
    )
    .unwrap();

    crate::TreeSummary::ensure(&staged, stats.generation).unwrap();
    let staged_wal = crate::compact::suffix_path(&path, ".delta.compact");
    drop(crate::DeltaIndex::replace_empty(&staged_wal, stats.generation).unwrap());
    replace_file(&staged_wal, &delta_path(&path)).unwrap();
    // The first recovery can publish the base; a later recovery must preserve
    // accepted edits in the now-matching generation WAL.
    let mut writer = crate::DeltaIndex::open(&delta_path(&path), stats.generation).unwrap();
    writer
        .apply(crate::DeltaChange::Upsert(rec("/accepted.txt", 9)))
        .unwrap();
    drop(writer);
    recover_streamed_publication(&path).unwrap();
    recover_streamed_publication(&path).unwrap();
    let (base, delta) = CompactIndex::open_with_delta_snapshot(&path).unwrap();
    assert_eq!(base.generation(), stats.generation);
    let delta = delta.unwrap();
    assert_eq!(delta.generation(), stats.generation);
    assert!(delta.upsert_for("/accepted.txt").unwrap().is_some());
    assert_eq!(base.records().unwrap()[0].path.as_ref(), "/new.txt");
    drop(base);
    clean(&path);
}

#[test]
fn rebuilt_cursor_seed_is_bound_to_published_generation() {
    let path = paths("cursor-publish");
    clean(&path);
    let cursors = vec![crate::NativeCursor {
        mount: "/mnt/a".into(),
        tree: 257,
        since: 42,
    }];
    let built = CompactIndex::rebuild_streamed_with_cursors(
        runs(vec![rec("/mnt/a/file", 1)], &path),
        &path,
        &cursors,
    )
    .unwrap();
    assert_eq!(
        crate::native_cursor::load_seed(&path, built.generation).unwrap(),
        cursors
    );
    assert!(crate::native_cursor::load_seed(&path, built.generation + 1).is_err());
    clean(&path);
}

#[test]
fn compact_publication_preserves_ingest_seed_across_generations() {
    let path = paths("cursor-compaction");
    clean(&path);
    let cursors = vec![crate::NativeCursor {
        mount: "/mnt/a".into(),
        tree: 257,
        since: 42,
    }];
    CompactIndex::rebuild_streamed_with_cursors(
        runs(vec![rec("/mnt/a/old", 1)], &path),
        &path,
        &cursors,
    )
    .unwrap();
    let staged = crate::compact::suffix_path(&path, ".compact");
    let built =
        CompactIndex::rebuild_streamed(runs(vec![rec("/mnt/a/new", 2)], &staged), &staged).unwrap();
    CompactIndex::publish(&staged, &path).unwrap();
    assert_eq!(
        crate::native_cursor::load_seed(&path, built.generation).unwrap(),
        cursors
    );
    clean(&path);
}
