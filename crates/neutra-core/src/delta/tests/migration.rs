use super::*;

#[test]
fn legacy_wal_replays_and_migrates_on_writer_open() {
    let path = std::env::temp_dir().join(format!("neutra-delta-v1-{}.wal", std::process::id()));
    remove_log(&path);
    let old = OldDeltaRecord {
        path: "/old".into(),
        size: 7,
        mtime: 0,
        mode: 0,
        kind: FileKind::File,
        fs: FsKind::Btrfs,
        native_id: 0,
        native_parent: 0,
        source: 0,
    };
    let payload = bincode::serialize(&OldDeltaChange::Upsert(old)).unwrap();
    let mut bytes = Vec::new();
    bytes.extend_from_slice(MAGIC_V1);
    bytes.extend_from_slice(&10u64.to_le_bytes());
    bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&crc32fast::hash(&payload).to_le_bytes());
    bytes.extend_from_slice(&payload);
    let mut file = crate::dir_summary::open_private_file(&path).unwrap();
    use std::io::Write as _;
    file.write_all(&bytes).unwrap();
    file.sync_all().unwrap();
    drop(file);
    let snapshot = DeltaIndex::open_snapshot(&path, 10).unwrap();
    assert!(snapshot.wal_v1);
    let mut count = 0;
    snapshot
        .for_each_upsert(|r| {
            count += 1;
            assert_eq!(r.disk_bytes(), 7);
            Ok(())
        })
        .unwrap();
    assert_eq!(count, 1);
    drop(snapshot);
    let writer = DeltaIndex::open(&path, 10).unwrap();
    assert!(!writer.wal_v1);
    drop(writer);
    let migrated = std::fs::read(&path).unwrap();
    assert_eq!(&migrated[..8], MAGIC);
    let snapshot = DeltaIndex::open_snapshot(&path, 10).unwrap();
    assert_eq!(snapshot.change_count().unwrap(), 1);
    remove_log(&path);
}

#[test]
fn legacy_migration_staging_failure_keeps_old_wal_intact() {
    let path = std::env::temp_dir().join(format!(
        "neutra-delta-v1-staging-{}.wal",
        std::process::id()
    ));
    remove_log(&path);
    let old = OldDeltaRecord {
        path: "/old".into(),
        size: 7,
        mtime: 0,
        mode: 0,
        kind: FileKind::File,
        fs: FsKind::Btrfs,
        native_id: 0,
        native_parent: 0,
        source: 0,
    };
    let payload = bincode::serialize(&OldDeltaChange::Upsert(old)).unwrap();
    let mut bytes = Vec::new();
    bytes.extend_from_slice(MAGIC_V1);
    bytes.extend_from_slice(&10u64.to_le_bytes());
    bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&crc32fast::hash(&payload).to_le_bytes());
    bytes.extend_from_slice(&payload);
    let mut file = crate::dir_summary::open_private_file(&path).unwrap();
    use std::io::Write as _;
    file.write_all(&bytes).unwrap();
    file.sync_all().unwrap();
    drop(file);
    let before = std::fs::read(&path).unwrap();
    // Force staged WAL creation to fail: the writer lock is held, the
    // authoritative legacy WAL must remain untouched.
    let staged = crate::compact::suffix_path(&path, ".migrate");
    let _ = std::fs::remove_file(&staged);
    let _ = std::fs::remove_dir_all(&staged);
    std::fs::create_dir_all(&staged).unwrap();
    assert!(DeltaIndex::open(&path, 10).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    // The legacy log still replays as before once staging can proceed.
    std::fs::remove_dir_all(&staged).unwrap();
    let writer = DeltaIndex::open(&path, 10).unwrap();
    assert!(!writer.wal_v1);
    drop(writer);
    let _ = std::fs::remove_file(&staged);
    remove_log(&path);
}
