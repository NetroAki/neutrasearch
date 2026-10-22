use super::*;
use crate::tests::{build_test_base, record, remove_store, store_paths};
use std::sync::{atomic::AtomicBool, Arc, RwLock};
use std::time::Duration;

fn fixture(label: &str) -> (DurableStore, std::path::PathBuf, std::path::PathBuf) {
    let (path, delta) = store_paths(label);
    build_test_base(
        &[record("/old.txt", 1), record("/folder/old.txt", 2)],
        &path,
    )
    .unwrap();
    let generation = CompactIndex::generation_on_disk(&path).unwrap();
    neutra_core::BrowserIndex::ensure(&path, generation).unwrap();
    let mut store = DurableStore::open_with_threshold(&path, 17).unwrap();
    store.background_compaction = true;
    store
        .apply_bounded(vec![DeltaChange::Upsert(record("/frozen.txt", 3))])
        .unwrap();
    (store, path, delta)
}

fn append_tail(store: &mut DurableStore) {
    let result = store
        .apply_bounded(vec![
            DeltaChange::Remove("/old.txt".into()),
            DeltaChange::RemovePrefix("/folder".into()),
            DeltaChange::Upsert(record("/folder/new.txt", 4)),
            DeltaChange::Upsert(record("/tail.txt", 5)),
            DeltaChange::Upsert(record("/frozen.txt", 99)),
        ])
        .unwrap();
    assert!(result.compacted.is_none());
}

fn assert_tail(store: &DurableStore) {
    let (hits, stats) = store.search(&Query::parse("ext:txt")).unwrap();
    assert_eq!(stats.matched, 3);
    let mut paths: Vec<_> = hits.iter().map(|hit| hit.record.path.as_ref()).collect();
    paths.sort();
    assert_eq!(paths, ["/folder/new.txt", "/frozen.txt", "/tail.txt"]);
    assert_eq!(
        hits.iter()
            .find(|hit| &*hit.record.path == "/frozen.txt")
            .unwrap()
            .record
            .size,
        99
    );
}

#[test]
fn live_events_and_catalog_remain_writable_during_frozen_compaction() {
    let (store, path, delta) = fixture("background-events");
    let store = Arc::new(RwLock::new(store));
    let frozen = store.read().unwrap().freeze_compaction().unwrap().unwrap();
    let (release, gate) = std::sync::mpsc::channel();
    let builder = std::thread::spawn(move || {
        gate.recv_timeout(Duration::from_secs(5)).unwrap();
        frozen.build().unwrap()
    });
    append_tail(&mut store.write().unwrap());
    assert_tail(&store.read().unwrap());
    release.send(()).unwrap();
    let prepared = builder.join().unwrap();
    store.write().unwrap().publish_compaction(prepared).unwrap();
    assert_tail(&store.read().unwrap());
    let generation = store.read().unwrap().delta.generation();
    let browser = neutra_core::BrowserIndex::open(&path, generation).unwrap();
    assert!(browser.covers_delta(&path).unwrap());
    assert!(browser.record_by_path("/folder/new.txt").unwrap().is_some());
    drop(browser);
    drop(store);
    let reopened = DurableStore::open(&path).unwrap();
    assert_tail(&reopened);
    drop(reopened);
    remove_store(&path, &delta);
}

#[test]
fn failed_background_build_leaves_the_live_pair_usable() {
    let (mut store, path, delta) = fixture("background-build-fails");
    let generation = store.delta.generation();
    let frozen = store.freeze_compaction().unwrap().unwrap();
    let blocked = append_suffix(&compaction_stage(&path), ".new");
    std::fs::create_dir(&blocked).unwrap();
    assert!(frozen.build().is_err());
    assert_eq!(store.delta.generation(), generation);
    append_tail(&mut store);
    assert_tail(&store);
    assert!(!compaction_marker(&path).exists());
    std::fs::remove_dir(blocked).unwrap();
    drop(store);
    remove_store(&path, &delta);
}

#[test]
fn startup_replay_writes_do_not_compact_or_change_the_pinned_base() {
    let (path, delta) = store_paths("startup-compaction-deferred");
    build_test_base(&[record("/old.txt", 1)], &path).unwrap();
    let generation = CompactIndex::generation_on_disk(&path).unwrap();
    neutra_core::BrowserIndex::ensure(&path, generation).unwrap();
    let mut store = DurableStore::open_with_threshold(&path, 1).unwrap();
    store.startup_compaction_deferred = true;

    let result = store
        .apply_bounded(vec![DeltaChange::Upsert(record("/replayed.txt", 2))])
        .unwrap();

    assert!(result.compacted.is_none());
    assert_eq!(CompactIndex::generation_on_disk(&path).unwrap(), generation);
    assert_eq!(store.base.as_ref().unwrap().generation(), generation);
    assert!(store.delta.upsert_for("/replayed.txt").unwrap().is_some());

    // Exercise the same locked handoff used by worker startup, then fold the
    // replay write through the newly enabled background-compaction path.
    let store = Arc::new(RwLock::new(store));
    assert!(worker::enable_background_compaction(&store).unwrap());
    {
        let guard = store.read().unwrap();
        assert!(!guard.startup_compaction_deferred);
        assert!(guard.background_compaction);
    }
    let prepared = store
        .read()
        .unwrap()
        .freeze_compaction()
        .unwrap()
        .unwrap()
        .build()
        .unwrap();
    store.write().unwrap().publish_compaction(prepared).unwrap();
    assert!(CompactIndex::generation_on_disk(&path).unwrap() != generation);
    assert!(store
        .read()
        .unwrap()
        .base
        .as_ref()
        .unwrap()
        .record_by_path_any_source("/replayed.txt")
        .unwrap()
        .is_some());

    drop(store);
    remove_store(&path, &delta);
}

#[test]
fn native_event_failure_during_a_fold_prevents_publication() {
    let (store, path, delta) = fixture("background-stale");
    let generation = store.delta.generation();
    let prepared = store.freeze_compaction().unwrap().unwrap().build().unwrap();
    let store = Arc::new(RwLock::new(store));
    write_stale_marker(&path, "native journal unavailable").unwrap();
    worker::commit_if_live(&store, prepared, &AtomicBool::new(true)).unwrap();
    assert_eq!(store.read().unwrap().delta.generation(), generation);
    assert_eq!(
        std::fs::read_to_string(stale_marker(&path)).unwrap(),
        "native journal unavailable"
    );
    drop(store);
    std::fs::remove_file(stale_marker(&path)).unwrap();
    remove_store(&path, &delta);
}

mod corruption;
mod recovery;
