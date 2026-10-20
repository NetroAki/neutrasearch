use neutra_core::{DeltaChange, DeltaIndex, FileKind, FileRecord, FsKind};

fn record(path: &str, size: u64) -> FileRecord {
    FileRecord {
        path: path.into(),
        size,
        disk: size,
        mtime: size as i64,
        mode: 0,
        kind: FileKind::File,
        fs: FsKind::Btrfs,
        native_id: size + 100,
        native_parent: 100,
        source: 0,
    }
}

/// A snapshot seeded from the writer's cache must still replay frames the
/// cache has not absorbed, and must ignore a cache that disagrees with the log.
#[cfg(unix)]
#[test]
fn snapshot_replays_the_tail_the_seed_cache_lacks() {
    let root = std::env::temp_dir().join(format!("neutra-seed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir(&root).unwrap();
    let path = root.join("base.delta");
    let cache = root.join(".delta.overlay/base.delta.resolved.sqlite");
    let mut writer = DeltaIndex::open(&path, 5).unwrap();
    writer
        .apply(DeltaChange::Upsert(record("/a.dat", 1)))
        .unwrap();
    writer.sync().unwrap();
    let behind = root.join("behind.sqlite");
    let cache_db = rusqlite::Connection::open(&cache).unwrap();
    cache_db
        .execute("VACUUM main INTO ?1", [behind.to_str().unwrap()])
        .unwrap();
    drop(cache_db);
    writer
        .apply(DeltaChange::Upsert(record("/b.dat", 2)))
        .unwrap();
    writer.sync().unwrap();
    drop(writer);
    // Rewind the cache to the state before /b.dat was logged.
    std::fs::copy(&behind, &cache).unwrap();

    let snapshot = DeltaIndex::open_snapshot(&path, 5).unwrap();
    assert!(snapshot.upsert_for("/a.dat").unwrap().is_some());
    assert!(snapshot.upsert_for("/b.dat").unwrap().is_some());
    assert_eq!(snapshot.change_count().unwrap(), 2);
    drop(snapshot);
    let _ = std::fs::remove_dir_all(&root);
}
