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
