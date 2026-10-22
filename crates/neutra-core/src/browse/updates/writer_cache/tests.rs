use super::*;
use crate::{BrowserIndex, FileKind, FileRecord, FsKind};

#[test]
fn writer_retains_one_validated_base_and_rejects_a_wrong_generation() {
    let temp = std::env::temp_dir().join(format!("neutra-writer-cache-{}", std::process::id()));
    std::fs::create_dir(&temp).unwrap();
    let path = temp.join("writer.nsx");
    let record = FileRecord {
        path: "/file".into(),
        size: 1,
        disk: 1,
        mtime: 1,
        mode: 0,
        kind: FileKind::File,
        fs: FsKind::Ext4,
        source: 0,
        native_id: 1,
        native_parent: 1,
    };
    let built = CompactIndex::build(&[record], &path).unwrap();
    BrowserIndex::ensure(&path, built.generation).unwrap();
    let first = open(&path, &built.generation.to_string()).unwrap();
    let next = open(&path, &built.generation.to_string()).unwrap();
    assert!(Arc::ptr_eq(&first.base, &next.base));
    assert!(Arc::ptr_eq(&first.decoded, &next.decoded));
    assert!(open(&path, "not-this-generation").is_err());
    let after = open(&path, &built.generation.to_string()).unwrap();
    assert!(Arc::ptr_eq(&first.base, &after.base));
    clear_for_test();
    drop((first, next, after));
    std::fs::remove_dir_all(temp).unwrap();
}
