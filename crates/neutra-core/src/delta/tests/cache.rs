use super::*;

#[test]
fn pre_fix_cache_is_replayed_instead_of_trusting_its_empty_rows() {
    let path =
        std::env::temp_dir().join(format!("neutra-delta-old-cache-{}.wal", std::process::id()));
    remove_log(&path);
    let mut writer = DeltaIndex::open(&path, 10).unwrap();
    writer
        .apply(DeltaChange::Upsert(record("/kept", 7)))
        .unwrap();
    writer.sync().unwrap();
    drop(writer);
    let cache = path.parent().unwrap().join(".delta.overlay").join(format!(
        "{}.resolved.sqlite",
        path.file_name().unwrap().to_string_lossy()
    ));
    let db = rusqlite::Connection::open(cache).unwrap();
    db.execute_batch("DELETE FROM changes; DELETE FROM meta WHERE k='format';")
        .unwrap();
    drop(db);
    let reopened = DeltaIndex::open(&path, 10).unwrap();
    assert_eq!(reopened.upsert_for("/kept").unwrap().unwrap().size, 7);
    drop(reopened);
    remove_log(&path);
}

#[test]
fn missing_wal_never_reuses_cached_rows_from_a_previous_generation() {
    let path = std::env::temp_dir().join(format!(
        "neutra-delta-missing-wal-{}.wal",
        std::process::id()
    ));
    remove_log(&path);
    let mut writer = DeltaIndex::open(&path, 10).unwrap();
    writer
        .apply(DeltaChange::Upsert(record("/old", 7)))
        .unwrap();
    writer.sync().unwrap();
    drop(writer);
    std::fs::remove_file(&path).unwrap();
    let mut rebuilt = DeltaIndex::open(&path, 11).unwrap();
    assert!(rebuilt.upsert_for("/old").unwrap().is_none());
    rebuilt
        .apply(DeltaChange::Upsert(record("/new", 9)))
        .unwrap();
    rebuilt.sync().unwrap();
    drop(rebuilt);
    let reopened = DeltaIndex::open(&path, 11).unwrap();
    assert!(reopened.upsert_for("/old").unwrap().is_none());
    assert_eq!(reopened.upsert_for("/new").unwrap().unwrap().size, 9);
    drop(reopened);
    remove_log(&path);
}
