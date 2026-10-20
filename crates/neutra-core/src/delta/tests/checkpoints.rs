use super::*;

#[cfg(unix)]
#[test]
fn repeated_checkpoints_preserve_unchanged_records_and_removals() {
    let path = std::env::temp_dir().join(format!(
        "neutra-delta-repeated-checkpoint-{}.wal",
        std::process::id()
    ));
    remove_log(&path);
    let mut writer = DeltaIndex::open(&path, 10).unwrap();
    writer
        .apply(DeltaChange::Upsert(record("/kept", 7)))
        .unwrap();
    writer.apply(DeltaChange::Remove("/gone".into())).unwrap();
    writer.checkpoint().unwrap();
    assert_eq!(writer.upsert_for("/kept").unwrap().unwrap().size, 7);
    assert!(writer.is_removed("/gone").unwrap());
    writer
        .apply(DeltaChange::Upsert(record("/added", 9)))
        .unwrap();
    writer.checkpoint().unwrap();
    drop(writer);
    let reopened = DeltaIndex::open(&path, 10).unwrap();
    assert_eq!(reopened.upsert_for("/kept").unwrap().unwrap().size, 7);
    assert_eq!(reopened.upsert_for("/added").unwrap().unwrap().size, 9);
    assert!(reopened.is_removed("/gone").unwrap());
    drop(reopened);
    let snapshot = DeltaIndex::open_snapshot(&path, 10).unwrap();
    assert_eq!(snapshot.upsert_for("/kept").unwrap().unwrap().size, 7);
    assert!(snapshot.is_removed("/gone").unwrap());
    drop(snapshot);
    remove_log(&path);
}

#[cfg(unix)]
#[test]
fn checkpoint_keeps_latest_state_and_invalidates_a_reader_even_after_log_growth() {
    let path = std::env::temp_dir().join(format!(
        "neutra-delta-checkpoint-{}.wal",
        std::process::id()
    ));
    remove_log(&path);
    let mut writer = DeltaIndex::open(&path, 10).unwrap();
    for size in 0..20 {
        writer
            .apply(DeltaChange::Upsert(record("/first", size)))
            .unwrap();
    }
    writer.apply(DeltaChange::Remove("/gone".into())).unwrap();
    writer.sync().unwrap();
    let mut reader = DeltaIndex::open_snapshot(&path, 10).unwrap();
    let original_bytes = writer.wal_bytes();
    writer.checkpoint().unwrap();
    assert!(writer.wal_bytes() < original_bytes);
    for size in 0..30 {
        writer
            .apply(DeltaChange::Upsert(record(&format!("/new{size}"), size)))
            .unwrap();
    }
    writer.sync().unwrap();
    assert!(writer.wal_bytes() > original_bytes);
    assert_eq!(
        reader.refresh().unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
    let current = DeltaIndex::open_snapshot(&path, 10).unwrap();
    assert_eq!(current.upsert_for("/first").unwrap().unwrap().size, 19);
    assert_eq!(current.change_count().unwrap(), 32);
    assert!(current.is_removed("/gone").unwrap());
    drop(writer);
    drop(current);
    drop(reader);
    remove_log(&path);
}
