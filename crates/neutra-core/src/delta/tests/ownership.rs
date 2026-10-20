use super::*;

#[test]
fn delta_index_is_send_and_sync_for_shared_watcher_threads() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<DeltaIndex>();
}

#[test]
fn permits_snapshots_but_rejects_a_second_writer() {
    let path = std::env::temp_dir().join(format!("neutra-delta-lock-{}.wal", std::process::id()));
    remove_log(&path);
    let mut writer = DeltaIndex::open(&path, 10).unwrap();
    writer
        .apply(DeltaChange::Upsert(record("/first", 1)))
        .unwrap();
    writer.sync().unwrap();
    let mut snapshot = DeltaIndex::open_snapshot(&path, 10).unwrap();
    assert_eq!(snapshot.generation(), 10);
    assert_eq!(snapshot.change_count().unwrap(), 1);
    let error = match DeltaIndex::open(&path, 10) {
        Ok(_) => panic!("second delta writer unexpectedly acquired the lock"),
        Err(error) => error,
    };
    assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
    writer
        .apply(DeltaChange::Upsert(record("/second", 2)))
        .unwrap();
    writer.sync().unwrap();
    assert!(snapshot.refresh().unwrap() > 0);
    assert_eq!(snapshot.change_count().unwrap(), 2);
    drop(snapshot);
    drop(writer);
    remove_log(&path);
}

#[test]
fn reset_rebinds_the_empty_wal_without_releasing_the_writer_lock() {
    let path = std::env::temp_dir().join(format!("neutra-delta-reset-{}.wal", std::process::id()));
    remove_log(&path);
    let mut delta = DeltaIndex::open(&path, 7).unwrap();
    let mut stale_snapshot = DeltaIndex::open_snapshot(&path, 7).unwrap();
    delta.apply(DeltaChange::Upsert(record("/a", 1))).unwrap();
    delta.sync().unwrap();

    delta.reset(8).unwrap();
    assert!(stale_snapshot.refresh().is_err());
    drop(stale_snapshot);
    assert_eq!(delta.generation(), 8);
    assert_eq!(delta.wal_bytes(), HEADER);
    assert_eq!(delta.change_count().unwrap(), 0);
    assert_eq!(std::fs::metadata(&path).unwrap().len(), HEADER);
    assert!(matches!(
        DeltaIndex::open(&path, 8),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock
    ));
    drop(delta);

    let reopened = DeltaIndex::open(&path, 8).unwrap();
    assert_eq!(reopened.change_count().unwrap(), 0);
    drop(reopened);
    assert!(DeltaIndex::open(&path, 7).is_err());
    remove_log(&path);
}
