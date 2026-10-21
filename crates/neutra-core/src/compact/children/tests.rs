use super::*;
use crate::{DeltaChange, FileKind, FsKind};

fn rec(path: &str, kind: FileKind, id: u64) -> FileRecord {
    FileRecord {
        path: path.into(),
        size: 1,
        disk: 1,
        mtime: 5,
        mode: 0,
        kind,
        fs: FsKind::Btrfs,
        native_id: id,
        native_parent: 0,
        source: 0,
    }
}

fn fixture(name: &str) -> (std::path::PathBuf, CompactIndex) {
    let records = vec![
        rec("/d", FileKind::Dir, 1),
        rec("/d/a", FileKind::Dir, 2),
        rec("/d/a-b.txt", FileKind::File, 3),
        rec("/d/a/deep", FileKind::Dir, 4),
        rec("/d/a/deep/x.txt", FileKind::File, 5),
        rec("/d/a.txt", FileKind::File, 6),
        rec("/d/a0", FileKind::File, 7),
        rec("/d/b", FileKind::Dir, 8),
        rec("/d/b/y.txt", FileKind::File, 9),
        rec("/e/z.txt", FileKind::File, 10),
    ];
    let path =
        std::env::temp_dir().join(format!("neutra-children-{name}-{}.idx", std::process::id()));
    CompactIndex::build(&records, &path).unwrap();
    let index = CompactIndex::open_fast(&path).unwrap();
    (path, index)
}

fn names(records: &[FileRecord]) -> Vec<String> {
    records.iter().map(|r| r.path.to_string()).collect()
}

#[test]
fn children_skip_subtrees_and_keep_odd_neighbours() {
    let (path, index) = fixture("kids");
    let kids = index.directory_children("/d", None).unwrap();
    assert_eq!(
        names(&kids),
        ["/d/a", "/d/a-b.txt", "/d/a.txt", "/d/a0", "/d/b"]
    );
    assert!(index
        .directory_children("/nothing", None)
        .unwrap()
        .is_empty());
    let _ = std::fs::remove_file(&path);
}

#[test]
#[cfg(not(windows))]
fn unix_literal_backslash_is_a_child_in_v7_ordered_indexes() {
    let records = vec![
        rec("/d", FileKind::Dir, 1),
        rec("/d/a0", FileKind::File, 2),
        rec("/d/a\\b", FileKind::File, 3),
    ];
    let path = std::env::temp_dir().join(format!(
        "neutra-children-backslash-{}.idx",
        std::process::id()
    ));
    CompactIndex::build(&records, &path).unwrap();
    let index = CompactIndex::open_fast(&path).unwrap();
    assert_eq!(
        names(&index.directory_children("/d", None).unwrap()),
        ["/d/a0", "/d/a\\b"]
    );
    drop(index);
    let _ = std::fs::remove_file(path);
}

#[test]
#[cfg(not(windows))]
fn legacy_v6_children_keep_backslash_separator_behavior() {
    let records = vec![
        rec("/d", FileKind::Dir, 1),
        rec("/d/a0", FileKind::File, 2),
        rec("/d/a\\b", FileKind::File, 3),
    ];
    let path = std::env::temp_dir().join(format!("neutra-children-v6-{}.idx", std::process::id()));
    CompactIndex::build(&records, &path).unwrap();
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[8..12].copy_from_slice(&6u32.to_le_bytes());
    let checksum_at = bytes.len() - crate::compact::CHECKSUM_BYTES;
    let checksum = crc32fast::hash(&bytes[..checksum_at]);
    bytes[checksum_at..].copy_from_slice(&checksum.to_le_bytes());
    std::fs::write(&path, bytes).unwrap();

    let index = CompactIndex::open_fast(&path).unwrap();
    assert_eq!(index.record_version, 6);
    assert!(index.subtree_records("/d", None, 100).is_err());
    assert_eq!(
        names(&index.directory_children("/d", None).unwrap()),
        ["/d/a0"]
    );
    drop(index);
    let _ = std::fs::remove_file(path);
}

#[test]
#[cfg(windows)]
fn windows_backslash_descendants_are_skipped_as_subtrees() {
    let records = vec![
        rec("C:/d", FileKind::Dir, 1),
        rec("C:/d/a", FileKind::Dir, 2),
        rec("C:/d/a/child.txt", FileKind::File, 3),
        rec("C:/d/b.txt", FileKind::File, 4),
    ];
    let path = std::env::temp_dir().join(format!(
        "neutra-children-windows-{}.idx",
        std::process::id()
    ));
    CompactIndex::build(&records, &path).unwrap();
    let index = CompactIndex::open_fast(&path).unwrap();
    assert_eq!(
        names(&index.directory_children("C:/d", None).unwrap()),
        ["C:/d/a", "C:/d/b.txt"]
    );
    drop(index);
    let _ = std::fs::remove_file(path);
}

#[test]
fn subtree_lists_everything_below_and_honours_the_limit() {
    let (path, index) = fixture("tree");
    let all = index.subtree_records("/d/a", None, 100).unwrap().unwrap();
    assert_eq!(names(&all), ["/d/a/deep", "/d/a/deep/x.txt"]);
    assert!(index.subtree_records("/d", None, 3).unwrap().is_none());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_delta_adds_and_hides_children() {
    let (path, index) = fixture("delta");
    let delta_path = path.with_extension("delta");
    let mut delta = DeltaIndex::open(&delta_path, index.generation()).unwrap();
    delta.apply(DeltaChange::Remove("/d/b".into())).unwrap();
    delta
        .apply(DeltaChange::Upsert(rec("/d/new.txt", FileKind::File, 11)))
        .unwrap();
    delta
        .apply(DeltaChange::Upsert(rec("/d/b/y2.txt", FileKind::File, 12)))
        .unwrap();
    let kids = index.directory_children("/d", Some(&delta)).unwrap();
    assert_eq!(
        names(&kids),
        ["/d/a", "/d/a-b.txt", "/d/a.txt", "/d/a0", "/d/new.txt"]
    );
    let below = index
        .subtree_records("/d/b", Some(&delta), 10)
        .unwrap()
        .unwrap();
    // A tombstone hides only its own path; callers expand a removed
    // directory into one tombstone per descendant.
    assert_eq!(names(&below), ["/d/b/y.txt", "/d/b/y2.txt"]);
    drop(delta);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&delta_path);
}
