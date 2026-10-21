use super::*;

#[test]
fn restart_preserves_nonempty_tail_at_each_publication_boundary() {
    for (label, marker, published) in [
        ("background-no-marker", false, false),
        ("background-with-marker", true, false),
        ("background-published", true, true),
    ] {
        recover_tail(label, marker, published);
    }
}

fn recover_tail(label: &str, marker: bool, published: bool) {
    let (mut store, path, delta) = fixture(label);
    let prepared = store.freeze_compaction().unwrap().unwrap().build().unwrap();
    append_tail(&mut store);
    let generation = prepared.built.generation;
    let staged_tail = delta
        .parent()
        .unwrap()
        .join(".delta.overlay")
        .join(format!("{label}.wal"));
    let bytes = store
        .delta
        .stage_compaction_tail(prepared.cursor, generation, &staged_tail)
        .unwrap();
    intent::write(&path, prepared.previous, generation, bytes).unwrap();
    store
        .delta
        .install_compaction_tail(&staged_tail, generation, bytes)
        .unwrap();
    if marker {
        write_compaction_marker(&compaction_marker(&path), generation).unwrap();
    }
    if published {
        CompactIndex::publish(&compaction_stage(&path), &path).unwrap();
    }
    drop(store);
    let recovered = DurableStore::open(&path).unwrap();
    assert_eq!(recovered.delta.generation(), generation);
    assert_tail(&recovered);
    assert!(!compaction_marker(&path).exists());
    let browser = neutra_core::BrowserIndex::open(&path, generation).unwrap();
    assert!(browser.covers_delta(&path).unwrap());
    drop(browser);
    drop(recovered);
    remove_store(&path, &delta);
}

#[test]
fn interrupted_publication_before_wal_replace_keeps_all_original_events() {
    let (mut store, path, delta) = fixture("background-before-replace");
    let prepared = store.freeze_compaction().unwrap().unwrap().build().unwrap();
    append_tail(&mut store);
    let generation = prepared.built.generation;
    let tail = delta
        .parent()
        .unwrap()
        .join(".delta.overlay")
        .join(format!("fold-tail-{}-{generation}", std::process::id()));
    let bytes = store
        .delta
        .stage_compaction_tail(prepared.cursor, generation, &tail)
        .unwrap();
    intent::write(&path, prepared.previous, generation, bytes).unwrap();
    drop(store);
    let recovered = DurableStore::open(&path).unwrap();
    assert_eq!(recovered.delta.generation(), prepared.previous);
    assert_tail(&recovered);
    assert!(!tail.exists());
    drop(recovered);
    remove_store(&path, &delta);
}

#[test]
fn published_base_recovers_its_cursor_before_watch_startup() {
    let (mut store, path, delta) = fixture("seed-after-base");
    let previous = store.base.as_ref().unwrap().generation();
    let cursors = vec![neutra_core::NativeCursor {
        mount: "/fixture".into(),
        tree: 257,
        since: 42,
    }];
    neutra_core::native_cursor::save_seed(&path, previous, &cursors).unwrap();
    let prepared = store.freeze_compaction().unwrap().unwrap().build().unwrap();
    let generation = prepared.built.generation;
    store.publish_compaction(prepared).unwrap();
    // Restore the durable evidence at the crash boundary after base replacement.
    let stage = compaction_stage(&path);
    std::fs::hard_link(&path, &stage).unwrap();
    let stage_dirs = DirectorySummary::path_for(&stage);
    if !stage_dirs.exists() {
        std::fs::hard_link(DirectorySummary::path_for(&path), &stage_dirs).unwrap();
    }
    let stage_tree = TreeSummary::path_for(&stage);
    if !stage_tree.exists() {
        std::fs::hard_link(TreeSummary::path_for(&path), &stage_tree).unwrap();
    }
    neutra_core::native_cursor::save_seed(&stage, generation, &cursors).unwrap();
    neutra_core::native_cursor::save_seed(&path, previous, &cursors).unwrap();
    write_compaction_marker(&compaction_marker(&path), generation).unwrap();
    drop(store);
    let recovered = DurableStore::open(&path).unwrap();
    assert_eq!(
        neutra_core::native_cursor::load_seed(&path, generation).unwrap(),
        cursors
    );
    drop(recovered);
    remove_store(&path, &delta);
}
