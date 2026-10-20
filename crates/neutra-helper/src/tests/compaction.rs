use super::*;

#[test]
fn durable_store_compacts_base_and_resets_delta_generation() {
    let (base_path, delta_path) = store_paths("compact");
    build_test_base(&[record("/old.txt", 1)], &base_path).unwrap();
    let original_generation = CompactIndex::open(&base_path).unwrap().generation();

    let mut store = DurableStore::open_with_threshold(&base_path, 17).unwrap();
    let applied = store
        .apply_bounded(vec![
            DeltaChange::Remove("/old.txt".into()),
            DeltaChange::Upsert(record("/new.txt", 2)),
        ])
        .unwrap();
    assert_eq!(applied.changes, 2);
    assert_eq!(applied.wal_bytes, 16);
    assert!(applied.compacted.is_some());
    let replacement_generation = store.base.as_ref().unwrap().generation();
    assert_ne!(replacement_generation, original_generation);
    assert_eq!(store.delta.generation(), replacement_generation);
    assert_eq!(store.delta.change_count().unwrap(), 0);
    let (hits, stats) = store.search(&Query::parse("ext:txt")).unwrap();
    assert_eq!(stats.matched, 1);
    assert_eq!(hits[0].record.path.as_ref(), "/new.txt");
    drop(store);

    let reopened = DurableStore::open(&base_path).unwrap();
    let (hits, stats) = reopened.search(&Query::parse("ext:txt")).unwrap();
    assert_eq!(stats.matched, 1);
    assert_eq!(hits[0].record.path.as_ref(), "/new.txt");
    drop(reopened);
    remove_store(&base_path, &delta_path);
}
