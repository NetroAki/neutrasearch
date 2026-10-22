use super::*;

#[test]
fn held_reader_rejects_a_rebuilt_catalog_with_different_base_addresses() {
    let root = std::env::temp_dir().join(format!("ns-reader-rebuild-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("index.nsx");
    let old = CompactIndex::build(&[record("/old", 1, 1, 1)], &path).unwrap();
    BrowserIndex::ensure(&path, old.generation).unwrap();
    let base = CompactIndex::open_fast(&path).unwrap();
    let held = BrowserIndex::open(&path, old.generation).unwrap();
    assert!(held.record_by_path("/old").unwrap().is_some());

    let new = CompactIndex::build(&[record("/new", 2, 2, 2)], &path).unwrap();
    BrowserIndex::ensure(&path, new.generation).unwrap();
    assert!(held.search(&base, &Query::default(), None).is_err());
    assert!(held.record_by_path("/old").is_err());
    assert!(held.directory_children("/").is_err());
    let current = BrowserIndex::open(&path, new.generation).unwrap();
    assert!(current.record_by_path("/new").unwrap().is_some());
    drop(current);
    drop(held);
    drop(base);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn cursor_publication_waits_for_a_competing_writer() {
    use std::sync::mpsc::{channel, RecvTimeoutError};
    use std::time::Duration;

    let root = std::env::temp_dir().join(format!("ns-cursor-lock-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("index.nsx");
    let generation = CompactIndex::build(&[record("/keep", 1, 1, 1)], &path)
        .unwrap()
        .generation;
    BrowserIndex::ensure(&path, generation).unwrap();
    let delta = DeltaIndex::open(&path.with_extension("delta"), generation).unwrap();
    BrowserIndex::mark_delta(&path, &delta).unwrap();
    let db = crate::sqlite::open(
        BrowserIndex::path_for(&path),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
    )
    .unwrap();
    db.execute_batch("BEGIN IMMEDIATE;").unwrap();
    let (started_tx, started_rx) = channel();
    let (result_tx, result_rx) = channel();
    let worker_path = path.clone();
    let worker = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        result_tx
            .send(BrowserIndex::mark_delta(&worker_path, &delta))
            .unwrap();
    });
    started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let premature = result_rx.recv_timeout(Duration::from_millis(100));
    db.execute_batch("ROLLBACK;").unwrap();
    assert!(matches!(premature, Err(RecvTimeoutError::Timeout)));
    result_rx
        .recv_timeout(Duration::from_secs(3))
        .unwrap()
        .unwrap();
    worker.join().unwrap();
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn catalog_recovery_restores_changed_added_removed_and_unchanged_records() {
    let path = std::env::temp_dir().join(format!("ns-catalog-recovery-{}.nsx", std::process::id()));
    let output = path.with_extension("recovered.delta");
    let _ = std::fs::remove_file(&output);
    CompactIndex::build(
        &[
            record("/keep", 1, 1, 1),
            record("/replace", 2, 2, 2),
            record("/gone", 3, 3, 3),
            record("/tail", 4, 4, 4),
        ],
        &path,
    )
    .unwrap();
    let base = CompactIndex::open_fast(&path).unwrap();
    let generation = base.generation();
    BrowserIndex::ensure(&path, generation).unwrap();
    BrowserIndex::apply_changes(
        &path,
        generation,
        &[
            DeltaChange::Remove("/gone".into()),
            DeltaChange::Upsert(record("/replace", 99, 99, 99)),
            DeltaChange::Upsert(record("/added", 5, 5, 5)),
        ],
    )
    .unwrap();
    let mut lost = DeltaIndex::open(&path.with_extension("delta"), generation).unwrap();
    lost.reset(generation).unwrap();
    BrowserIndex::mark_delta(&path, &lost).unwrap();
    drop(lost);
    BrowserIndex::recover_delta(&path, &output).unwrap();
    let recovered = DeltaIndex::open_snapshot(&output, generation).unwrap();
    let (hits, stats) = base
        .search_with_delta(&Query::default(), &recovered)
        .unwrap();
    assert_eq!(stats.matched, 4);
    assert!(!hits.iter().any(|hit| &*hit.record.path == "/gone"));
    assert_eq!(recovered.upsert_for("/replace").unwrap().unwrap().size, 99);
    assert!(hits.iter().any(|hit| &*hit.record.path == "/keep"));
    assert!(hits.iter().any(|hit| &*hit.record.path == "/tail"));
    assert!(hits.iter().any(|hit| &*hit.record.path == "/added"));
    let mut pending = DeltaIndex::open(&path.with_extension("delta"), generation).unwrap();
    pending.apply(DeltaChange::Remove("/keep".into())).unwrap();
    pending.sync().unwrap();
    drop(pending);
    let another = path.with_extension("unsafe.delta");
    assert!(BrowserIndex::recover_delta(&path, &another).is_err());
    assert!(!another.exists());
}
