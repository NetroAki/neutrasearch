use super::*;

#[test]
fn damaged_committed_tail_is_never_replaced_with_an_empty_log() {
    let (mut store, path, delta) = fixture("background-damaged-tail");
    let prepared = store.freeze_compaction().unwrap().unwrap().build().unwrap();
    append_tail(&mut store);
    let generation = prepared.built.generation;
    let tail = delta
        .parent()
        .unwrap()
        .join(".delta.overlay")
        .join("damaged-tail.wal");
    let bytes = store
        .delta
        .stage_compaction_tail(prepared.cursor, generation, &tail)
        .unwrap();
    intent::write(&path, prepared.previous, generation, bytes).unwrap();
    store
        .delta
        .install_compaction_tail(&tail, generation, bytes)
        .unwrap();
    drop(store);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&delta)
        .unwrap()
        .set_len(8)
        .unwrap();
    assert!(DurableStore::open(&path).is_err());
    assert_eq!(std::fs::metadata(&delta).unwrap().len(), 8);
    intent::finish(&path, &delta).unwrap();
    remove_store(&path, &delta);
}
