use super::*;
use crate::browse::short_counts::{key, matched, ready, SLOTS};

fn record(path: &str) -> FileRecord {
    FileRecord {
        path: path.into(),
        size: 0,
        disk: 0,
        mtime: 0,
        mode: 0,
        kind: crate::FileKind::File,
        fs: crate::mounts::FsKind::Ext4,
        native_id: 0,
        native_parent: 0,
        source: 0,
    }
}

#[test]
fn repeated_names_update_each_counter_once() {
    let db = Connection::open_in_memory().unwrap();
    Counts::default().publish(&db, true).unwrap();
    let before = db.total_changes();
    let mut batch = Batch::default();
    for n in 0..256 {
        batch.change(None, Some(&record(&format!("/{n}/Alpha"))));
    }
    batch.finish(&db).unwrap();
    assert_eq!(matched(&db, key("al").unwrap()).unwrap(), 256);
    assert!(db.total_changes() - before <= SLOTS as u64);
}

#[test]
fn renames_saves_and_removals_preserve_exact_counts_and_path_mode() {
    let db = Connection::open_in_memory().unwrap();
    let mut initial = Counts::default();
    initial.add_name("Alpha", 2);
    initial.publish(&db, true).unwrap();
    let old = record("/a/Alpha");
    let new = record("C:/Beta");
    let mut batch = Batch::default();
    batch.change(Some(&old), Some(&new));
    batch.change(Some(&old), Some(&old));
    batch.change(Some(&old), None);
    batch.finish(&db).unwrap();
    assert_eq!(matched(&db, key("al").unwrap()).unwrap(), 0);
    assert_eq!(matched(&db, key("be").unwrap()).unwrap(), 1);
    assert_eq!(ready(&db).unwrap(), Some(false));
}

#[test]
fn failed_transaction_rolls_back_counts_and_path_mode() {
    let mut db = Connection::open_in_memory().unwrap();
    Counts::default().publish(&db, true).unwrap();
    {
        let tx = db.transaction().unwrap();
        let mut batch = Batch::default();
        batch.change(None, Some(&record("C:/Beta")));
        batch.finish(&tx).unwrap();
        assert_eq!(matched(&tx, key("be").unwrap()).unwrap(), 1);
    }
    assert_eq!(matched(&db, key("be").unwrap()).unwrap(), 0);
    assert_eq!(ready(&db).unwrap(), Some(true));
}
