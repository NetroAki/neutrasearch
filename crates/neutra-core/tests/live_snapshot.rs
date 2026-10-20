use neutra_core::{
    CompactIndex, DeltaChange, DeltaIndex, FileKind, FileRecord, FsKind, Query, SpillAccumulator,
};

fn record(path: &str, size: u64) -> FileRecord {
    FileRecord {
        path: path.into(),
        size,
        disk: size,
        mtime: size as i64,
        mode: 0,
        kind: FileKind::File,
        fs: FsKind::Btrfs,
        native_id: size,
        native_parent: 0,
        source: 0,
    }
}

#[test]
fn live_snapshot_is_immutable_and_does_not_block_the_writer() {
    let root = std::env::temp_dir().join(format!("ns-live-snapshot-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("base.nsx");
    let mut spill = SpillAccumulator::begin(&path).unwrap();
    spill
        .push_batch(vec![record("/old/file", 1), record("/stable/file", 2)])
        .unwrap();
    CompactIndex::rebuild_streamed(spill.finish().unwrap(), &path).unwrap();
    let base = CompactIndex::open_fast(&path).unwrap();
    let wal = path.with_extension("delta");
    let mut writer = DeltaIndex::open(&wal, base.generation()).unwrap();
    writer
        .apply(DeltaChange::Upsert(record("/old/file", 3)))
        .unwrap();
    writer.sync().unwrap();
    let mut snapshot = DeltaIndex::open_search_snapshot(&wal, base.generation()).unwrap();
    writer
        .apply_batch([
            DeltaChange::RemovePrefix("/old".into()),
            DeltaChange::Upsert(record("/new/file", 4)),
        ])
        .unwrap();
    writer.sync().unwrap();
    assert_eq!(snapshot.upsert_for("/old/file").unwrap().unwrap().size, 3);
    assert!(snapshot.upsert_for("/new/file").unwrap().is_none());
    let query = Query::parse("file");
    let before = base.search_with_delta(&query, &snapshot).unwrap().0;
    assert_eq!(before.len(), 2);
    assert!(before
        .iter()
        .any(|hit| hit.record.path.as_ref() == "/old/file" && hit.record.size == 3));
    assert_eq!(
        snapshot
            .apply(DeltaChange::Remove("/stable/file".into()))
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::PermissionDenied
    );
    snapshot.refresh().unwrap();
    assert!(snapshot.upsert_for("/old/file").unwrap().is_none());
    assert!(snapshot.is_removed("/old/file").unwrap());
    let after = base.search_with_delta(&query, &snapshot).unwrap().0;
    assert_eq!(after.len(), 2);
    assert!(after
        .iter()
        .any(|hit| hit.record.path.as_ref() == "/new/file"));
    assert!(!after.iter().any(|hit| hit.record.path.starts_with("/old/")));
    let entries = std::fs::read_dir(root.join(".delta.overlay")).unwrap();
    assert!(entries
        .map(Result::unwrap)
        .all(|entry| !entry.file_name().to_string_lossy().contains("snapshot-")));
    writer.reset(base.generation().wrapping_add(1)).unwrap();
    assert!(snapshot.refresh().is_err());
    assert!(snapshot.upsert_for("/new/file").unwrap().is_some());
    drop(snapshot);
    drop(writer);
    drop(base);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn live_snapshot_rejects_a_cache_that_lags_the_authoritative_log() {
    use std::io::Write;
    let root = std::env::temp_dir().join(format!("ns-live-lag-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("base.delta");
    let mut writer = DeltaIndex::open(&path, 10).unwrap();
    writer.apply(DeltaChange::Upsert(record("/a", 1))).unwrap();
    writer.sync().unwrap();
    drop(writer);
    let payload = bincode::serialize(&DeltaChange::Upsert(record("/b", 2))).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    file.write_all(&(payload.len() as u32).to_le_bytes())
        .unwrap();
    file.write_all(&crc32fast::hash(&payload).to_le_bytes())
        .unwrap();
    file.write_all(&payload).unwrap();
    file.sync_all().unwrap();
    assert_eq!(
        DeltaIndex::open_search_snapshot(&path, 10)
            .err()
            .unwrap()
            .kind(),
        std::io::ErrorKind::WouldBlock
    );
    let private = DeltaIndex::open_snapshot(&path, 10).unwrap();
    assert!(private.upsert_for("/a").unwrap().is_some());
    assert!(private.upsert_for("/b").unwrap().is_some());
    drop(private);
    drop(file);
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn live_snapshot_rejects_aliased_or_shared_overlay_roots() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let root = std::env::temp_dir().join(format!("ns-live-root-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("base.delta");
    let mut writer = DeltaIndex::open(&path, 10).unwrap();
    writer
        .apply(DeltaChange::Upsert(record("/file", 1)))
        .unwrap();
    writer.sync().unwrap();
    drop(writer);
    let overlay = root.join(".delta.overlay");
    let moved = root.join("moved-overlay");
    std::fs::rename(&overlay, &moved).unwrap();
    symlink(&moved, &overlay).unwrap();
    assert_eq!(
        DeltaIndex::open_search_snapshot(&path, 10)
            .err()
            .unwrap()
            .kind(),
        std::io::ErrorKind::PermissionDenied
    );
    std::fs::remove_file(&overlay).unwrap();
    std::fs::rename(&moved, &overlay).unwrap();
    std::fs::set_permissions(&overlay, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        DeltaIndex::open_search_snapshot(&path, 10)
            .err()
            .unwrap()
            .kind(),
        std::io::ErrorKind::PermissionDenied
    );
    std::fs::set_permissions(&overlay, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(DeltaIndex::open_search_snapshot(&path, 10).is_ok());
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn live_snapshot_preserves_non_utf8_index_names() {
    use std::os::unix::ffi::OsStrExt;
    let root = std::env::temp_dir().join(format!("ns-live-name-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join(std::ffi::OsStr::from_bytes(b"base-\xff.delta"));
    let mut writer = DeltaIndex::open(&path, 10).unwrap();
    writer
        .apply(DeltaChange::Upsert(record("/file", 1)))
        .unwrap();
    writer.sync().unwrap();
    let snapshot = DeltaIndex::open_search_snapshot(&path, 10).unwrap();
    assert!(snapshot.upsert_for("/file").unwrap().is_some());
    drop(snapshot);
    drop(writer);
    std::fs::remove_dir_all(root).unwrap();
}
