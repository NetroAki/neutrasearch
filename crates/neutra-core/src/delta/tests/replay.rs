use super::*;

#[test]
fn wal_replays_upserts_and_tombstones() {
    let path = std::env::temp_dir().join(format!("neutra-delta-{}.wal", std::process::id()));
    remove_log(&path);
    {
        let mut delta = DeltaIndex::open_with_threshold(&path, 7, 1).unwrap();
        delta.apply(DeltaChange::Upsert(record("/a", 1))).unwrap();
        delta.apply(DeltaChange::Upsert(record("/b", 2))).unwrap();
        delta.apply(DeltaChange::Remove("/a".into())).unwrap();
        delta.sync().unwrap();
        assert!(delta.needs_compaction());
    }
    let delta = DeltaIndex::open(&path, 7).unwrap();
    assert!(delta.is_removed("/a").unwrap());
    assert_eq!(delta.upsert_for("/b").unwrap().unwrap().path.as_ref(), "/b");
    drop(delta);
    assert!(DeltaIndex::open(&path, 8).is_err());
    remove_log(&path);
}
