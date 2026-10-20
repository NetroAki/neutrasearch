use super::*;

#[cfg(unix)]
#[test]
fn a_ready_catalog_folds_changes_into_the_base_and_keeps_browse_coverage() {
    let (base_path, delta_path) = store_paths("catalog-fold");
    let records: Vec<_> = (0..100)
        .map(|n| record(&format!("/old{n:03}.txt"), n))
        .collect();
    build_test_base(&records, &base_path).unwrap();
    let generation = CompactIndex::generation_on_disk(&base_path).unwrap();
    neutra_core::BrowserIndex::ensure(&base_path, generation).unwrap();
    let mut store = DurableStore::open_with_threshold(&base_path, 17).unwrap();
    let result = store
        .apply_bounded(vec![
            neutra_core::DeltaChange::Remove("/old050.txt".into()),
            neutra_core::DeltaChange::Upsert(record("/saved.txt", 9)),
        ])
        .unwrap();
    assert!(result.compacted.is_some());
    assert_eq!(result.wal_bytes, store.delta.wal_bytes());
    let folded = CompactIndex::generation_on_disk(&base_path).unwrap();
    assert_ne!(folded, generation);
    let base = CompactIndex::open_fast(&base_path).unwrap();
    let browser = neutra_core::BrowserIndex::open(&base_path, folded).unwrap();
    assert!(browser.covers_delta(&base_path).unwrap());
    let hits = browser
        .search(&base, &neutra_core::Query::default(), None)
        .unwrap()
        .0;
    assert!(hits.iter().any(|hit| &*hit.record.path == "/saved.txt"));
    assert!(!hits.iter().any(|hit| &*hit.record.path == "/old050.txt"));
    assert_eq!(hits.len(), 100);
    drop(browser);
    drop(base);
    drop(store);
    remove_store(&base_path, &delta_path);
    for suffix in [
        ".browse",
        ".browse.base",
        ".browse.cols",
        ".browse.cols.new",
        ".browse.cols.new.runs",
        ".browse.lock",
        ".browse-wal",
        ".browse-shm",
    ] {
        let _ = std::fs::remove_file(store::append_suffix(&base_path, suffix));
    }
}

#[test]
fn durable_store_syncs_and_searches_delta() {
    let (base_path, delta_path) = store_paths("store");
    build_test_base(&[record("/old.txt", 1)], &base_path).unwrap();

    let mut store = DurableStore::open(&base_path).unwrap();
    let applied = store
        .apply(vec![
            DeltaChange::Remove("/old.txt".into()),
            DeltaChange::Upsert(record("/new.txt", 2)),
        ])
        .unwrap();
    assert_eq!(applied.0, 2);
    let (hits, stats) = store.search(&Query::parse("ext:txt")).unwrap();
    assert_eq!(stats.matched, 1);
    assert_eq!(hits[0].record.path.as_ref(), "/new.txt");
    drop(store);

    let reopened = DurableStore::open(&base_path).unwrap();
    let (hits, _) = reopened.search(&Query::parse("new")).unwrap();
    assert_eq!(hits[0].record.path.as_ref(), "/new.txt");
    drop(reopened);
    remove_store(&base_path, &delta_path);
}

#[test]
fn ignores_unpublished_partial_marker_and_staged_base() {
    let (base_path, delta_path) = store_paths("partial-marker");
    build_test_base(&[record("/old.txt", 1)], &base_path).unwrap();
    drop(DurableStore::open(&base_path).unwrap());
    let staged_path = compaction_stage(&base_path);
    build_test_base(&[record("/new.txt", 2)], &staged_path).unwrap();
    std::fs::write(compaction_marker_temp(&base_path), [1, 2, 3]).unwrap();

    let store = DurableStore::open(&base_path).unwrap();
    let (hits, stats) = store.search(&Query::parse("ext:txt")).unwrap();
    assert_eq!(stats.matched, 1);
    assert_eq!(hits[0].record.path.as_ref(), "/old.txt");
    assert!(!staged_path.exists());
    assert!(!compaction_marker_temp(&base_path).exists());
    drop(store);
    remove_store(&base_path, &delta_path);
}

#[test]
fn recovers_compaction_after_marker_before_wal_reset() {
    let (base_path, delta_path) = store_paths("recover-before-reset");
    build_test_base(&[record("/old.txt", 1)], &base_path).unwrap();
    let mut store = DurableStore::open(&base_path).unwrap();
    store
        .apply(vec![
            DeltaChange::Remove("/old.txt".into()),
            DeltaChange::Upsert(record("/new.txt", 2)),
        ])
        .unwrap();
    drop(store);

    let staged_path = compaction_stage(&base_path);
    let built = build_test_base(&[record("/new.txt", 2)], &staged_path).unwrap();
    write_compaction_marker(&compaction_marker(&base_path), built.generation).unwrap();

    let recovered = DurableStore::open(&base_path).unwrap();
    let (hits, stats) = recovered.search(&Query::parse("ext:txt")).unwrap();
    assert_eq!(stats.matched, 1);
    assert_eq!(hits[0].record.path.as_ref(), "/new.txt");
    assert_eq!(recovered.delta.generation(), built.generation);
    assert!(!compaction_marker(&base_path).exists());
    drop(recovered);
    remove_store(&base_path, &delta_path);
}

#[test]
fn recovers_compaction_after_wal_reset_before_base_publish() {
    let (base_path, delta_path) = store_paths("recover-after-reset");
    build_test_base(&[record("/old.txt", 1)], &base_path).unwrap();
    let mut store = DurableStore::open(&base_path).unwrap();
    store
        .apply(vec![
            DeltaChange::Remove("/old.txt".into()),
            DeltaChange::Upsert(record("/new.txt", 2)),
        ])
        .unwrap();
    let staged_path = compaction_stage(&base_path);
    let built = build_test_base(&[record("/new.txt", 2)], &staged_path).unwrap();
    write_compaction_marker(&compaction_marker(&base_path), built.generation).unwrap();
    store.delta.reset(built.generation).unwrap();
    drop(store);

    let recovered = DurableStore::open(&base_path).unwrap();
    let (hits, stats) = recovered.search(&Query::parse("ext:txt")).unwrap();
    assert_eq!(stats.matched, 1);
    assert_eq!(hits[0].record.path.as_ref(), "/new.txt");
    assert_eq!(recovered.delta.generation(), built.generation);
    assert!(!compaction_marker(&base_path).exists());
    drop(recovered);
    remove_store(&base_path, &delta_path);
}
