use super::*;

#[test]
fn recovers_when_marker_is_lost_with_a_torn_reset_wal() {
    for length in [0, 1, 7, 15] {
        let (base_path, delta_path) = store_paths(&format!("recover-marker-lost-{length}"));
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
        let marker = compaction_marker(&base_path);
        write_compaction_marker(&marker, built.generation).unwrap();
        store.delta.reset(built.generation).unwrap();
        drop(store);
        std::fs::remove_file(marker).unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&delta_path)
            .unwrap()
            .set_len(length)
            .unwrap();

        let recovered = DurableStore::open(&base_path).unwrap();
        let (hits, stats) = recovered.search(&Query::parse("ext:txt")).unwrap();
        assert_eq!(stats.matched, 1);
        assert_eq!(hits[0].record.path.as_ref(), "/new.txt");
        assert_eq!(recovered.delta.generation(), built.generation);
        drop(recovered);
        remove_store(&base_path, &delta_path);
    }
}

#[test]
fn recovers_torn_wal_header_when_staged_base_is_verified() {
    for length in [0, 1, 7, 15] {
        let (base_path, delta_path) = store_paths(&format!("recover-torn-{length}"));
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
        std::fs::OpenOptions::new()
            .write(true)
            .open(&delta_path)
            .unwrap()
            .set_len(length)
            .unwrap();

        let recovered = DurableStore::open(&base_path).unwrap();
        let (hits, stats) = recovered.search(&Query::parse("ext:txt")).unwrap();
        assert_eq!(stats.matched, 1);
        assert_eq!(hits[0].record.path.as_ref(), "/new.txt");
        assert_eq!(recovered.delta.generation(), built.generation);
        drop(recovered);
        remove_store(&base_path, &delta_path);
    }
}

#[test]
fn corrupt_complete_wal_is_not_discarded_during_compaction_recovery() {
    let (base_path, delta_path) = store_paths("reject-corrupt-recovery-wal");
    build_test_base(&[record("/old.txt", 1)], &base_path).unwrap();
    let mut store = DurableStore::open(&base_path).unwrap();
    let staged_path = compaction_stage(&base_path);
    let built = build_test_base(&[record("/new.txt", 2)], &staged_path).unwrap();
    write_compaction_marker(&compaction_marker(&base_path), built.generation).unwrap();
    store.delta.reset(built.generation).unwrap();
    store
        .delta
        .apply(DeltaChange::Upsert(record("/later.txt", 3)))
        .unwrap();
    drop(store);

    let mut wal = std::fs::read(&delta_path).unwrap();
    *wal.last_mut().unwrap() ^= 0xff;
    std::fs::write(&delta_path, wal).unwrap();
    let error = DurableStore::open(&base_path)
        .err()
        .expect("corrupt complete WAL must fail closed");
    assert!(format!("{error:#}").contains("checksum mismatch"));
    assert!(compaction_marker(&base_path).exists());

    remove_store(&base_path, &delta_path);
}
