use super::*;
use crate::tests::{build_test_base, record, remove_store, store_paths};
use neutra_core::DeltaChange;

#[test]
fn an_older_replay_cannot_replace_a_live_save_or_delete() {
    let (path, delta) = store_paths("replay-live-fence");
    build_test_base(&[record("/saved.txt", 1), record("/deleted.txt", 1)], &path).unwrap();
    let mut writer = DurableStore::open(&path).unwrap();
    writer.background_compaction = true;
    let cursor = writer.delta.compaction_cursor();
    let plan = Pending::new(vec![
        DeltaChange::Upsert(record("/saved.txt", 2)),
        DeltaChange::Upsert(record("/deleted.txt", 2)),
        DeltaChange::Upsert(record("/untouched.txt", 3)),
    ]);
    writer
        .apply_bounded(vec![
            DeltaChange::Upsert(record("/saved.txt", 99)),
            DeltaChange::Remove("/deleted.txt".into()),
        ])
        .unwrap();
    let store = Arc::new(RwLock::new(writer));
    assert!(apply(&store, &WriteGate::default(), plan, cursor).unwrap());
    let guard = store.read().unwrap();
    assert_eq!(
        guard.delta.upsert_for("/saved.txt").unwrap().unwrap().size,
        99
    );
    assert!(guard.delta.is_removed("/deleted.txt").unwrap());
    assert_eq!(
        guard
            .delta
            .upsert_for("/untouched.txt")
            .unwrap()
            .unwrap()
            .size,
        3
    );
    drop(guard);
    drop(store);
    remove_store(&path, &delta);
}

#[test]
fn a_folder_conflict_requires_retry_instead_of_removing_new_live_files() {
    let (path, delta) = store_paths("replay-prefix-fence");
    build_test_base(&[record("/folder/old.txt", 1)], &path).unwrap();
    let mut writer = DurableStore::open(&path).unwrap();
    writer.background_compaction = true;
    let cursor = writer.delta.compaction_cursor();
    let plan = Pending::new(vec![
        DeltaChange::RemovePrefix("/folder".into()),
        DeltaChange::Upsert(record("/folder/outdated.txt", 2)),
    ]);
    writer
        .apply_bounded(vec![DeltaChange::Upsert(record("/folder/current.txt", 99))])
        .unwrap();
    let store = Arc::new(RwLock::new(writer));
    assert!(!apply(&store, &WriteGate::default(), plan, cursor).unwrap());
    let guard = store.read().unwrap();
    assert_eq!(
        guard
            .delta
            .upsert_for("/folder/current.txt")
            .unwrap()
            .unwrap()
            .size,
        99
    );
    assert!(guard
        .delta
        .upsert_for("/folder/outdated.txt")
        .unwrap()
        .is_none());
    assert!(!guard.delta.is_removed("/folder/old.txt").unwrap());
    drop(guard);
    drop(store);
    remove_store(&path, &delta);
}
