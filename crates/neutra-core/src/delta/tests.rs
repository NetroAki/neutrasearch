use super::*;
use crate::{FileKind, FsKind};
fn record(path: &str, size: u64) -> FileRecord {
    FileRecord {
        path: path.into(),
        size,
        mtime: 0,
        mode: 0,
        kind: FileKind::File,
        fs: FsKind::Btrfs,
        native_id: 0,
        native_parent: 0,
        source: 0,
        disk: 0,
    }
}
fn remove_log(path: &Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(lock_path(path));
}

#[test]
fn delta_index_is_send_and_sync_for_shared_watcher_threads() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<DeltaIndex>();
}

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
fn complete_frame_with_bad_checksum_fails_closed() {
    let path =
        std::env::temp_dir().join(format!("neutra-delta-corrupt-{}.wal", std::process::id()));
    remove_log(&path);
    let mut delta = DeltaIndex::open(&path, 11).unwrap();
    delta.apply(DeltaChange::Upsert(record("/a", 1))).unwrap();
    delta.sync().unwrap();
    drop(delta);

    let mut bytes = std::fs::read(&path).unwrap();
    *bytes.last_mut().unwrap() ^= 0x80;
    std::fs::write(&path, bytes).unwrap();
    assert!(DeltaIndex::open(&path, 11).is_err());
    remove_log(&path);
}

#[cfg(unix)]
#[test]
fn same_length_wal_rewrite_invalidates_the_persistent_cursor_and_checks_crc() {
    let path =
        std::env::temp_dir().join(format!("neutra-delta-rewrite-{}.wal", std::process::id()));
    remove_log(&path);
    {
        let mut delta = DeltaIndex::open(&path, 15).unwrap();
        delta.apply(DeltaChange::Upsert(record("/a", 1))).unwrap();
    }
    let mut bytes = std::fs::read(&path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0x80;
    std::fs::write(&path, bytes).unwrap();
    assert!(DeltaIndex::open(&path, 15).is_err());
    remove_log(&path);
    let overlay = path.parent().unwrap().join(".delta.overlay");
    let db = overlay.join(format!(
        "{}.resolved.sqlite",
        path.file_name().unwrap().to_string_lossy()
    ));
    let _ = std::fs::remove_file(db);
}

#[cfg(unix)]
#[test]
fn persistent_overlay_rejects_symlink_and_hardlink_database_files() {
    use std::os::unix::fs::symlink;
    let path = std::env::temp_dir().join(format!(
        "neutra-delta-cache-safe-{}.wal",
        std::process::id()
    ));
    remove_log(&path);
    let root = path.parent().unwrap().join(".delta.overlay");
    std::fs::create_dir_all(&root).unwrap();
    let db = root.join(format!(
        "{}.resolved.sqlite",
        path.file_name().unwrap().to_string_lossy()
    ));
    let victim = root.join(format!("victim-{}", std::process::id()));
    let _ = std::fs::remove_file(&db);
    let _ = std::fs::remove_file(&victim);
    std::fs::write(&victim, b"preserve").unwrap();
    symlink(&victim, &db).unwrap();
    assert!(DeltaIndex::open(&path, 17).is_err());
    assert_eq!(std::fs::read(&victim).unwrap(), b"preserve");
    std::fs::remove_file(&db).unwrap();
    std::fs::hard_link(&victim, &db).unwrap();
    assert!(DeltaIndex::open(&path, 17).is_err());
    assert_eq!(std::fs::read(&victim).unwrap(), b"preserve");
    std::fs::remove_file(&db).unwrap();
    std::fs::remove_file(&victim).unwrap();
    remove_log(&path);
}

#[test]
fn torn_tail_is_truncated_before_new_appends() {
    let path = std::env::temp_dir().join(format!("neutra-delta-tail-{}.wal", std::process::id()));
    remove_log(&path);
    {
        let mut delta = DeltaIndex::open(&path, 9).unwrap();
        delta.apply(DeltaChange::Upsert(record("/a", 1))).unwrap();
        delta.sync().unwrap();
    }
    let verified_bytes = std::fs::metadata(&path).unwrap().len();
    let mut torn = OpenOptions::new().write(true).open(&path).unwrap();
    torn.seek(SeekFrom::End(0)).unwrap();
    torn.write_all(&[4, 0]).unwrap();
    drop(torn);
    {
        let mut recovered = DeltaIndex::open(&path, 9).unwrap();
        assert_eq!(recovered.wal_bytes(), verified_bytes);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), verified_bytes);
        recovered
            .apply(DeltaChange::Upsert(record("/b", 2)))
            .unwrap();
        recovered.sync().unwrap();
    }
    let reopened = DeltaIndex::open(&path, 9).unwrap();
    assert_eq!(reopened.change_count().unwrap(), 2);
    drop(reopened);
    remove_log(&path);
}

#[cfg(unix)]
#[test]
fn writer_refuses_symlinked_wal_and_lock_files() {
    use std::os::unix::fs::symlink;

    let path =
        std::env::temp_dir().join(format!("neutra-delta-symlink-{}.wal", std::process::id()));
    let victim = path.with_extension("victim");
    remove_log(&path);
    let _ = std::fs::remove_file(&victim);
    std::fs::write(&victim, b"do not truncate").unwrap();
    symlink(&victim, &path).unwrap();
    assert!(DeltaIndex::replace_empty(&path, 7).is_err());
    assert_eq!(std::fs::read(&victim).unwrap(), b"do not truncate");
    std::fs::remove_file(&path).unwrap();
    remove_log(&path);

    let lock = lock_path(&path);
    symlink(&victim, &lock).unwrap();
    assert!(DeltaIndex::open(&path, 7).is_err());
    assert_eq!(std::fs::read(&victim).unwrap(), b"do not truncate");
    std::fs::remove_file(lock).unwrap();
    let _ = std::fs::remove_file(path);
    std::fs::remove_file(victim).unwrap();
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
