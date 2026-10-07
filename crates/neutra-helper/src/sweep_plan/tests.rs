use super::*;
use crate::sweep_plan::Churn;

#[derive(Default)]
struct Fake {
    dirs: HashMap<u64, String>,
    kids: HashMap<u64, Vec<Child>>,
    inodes: HashMap<u64, Inode>,
    refs: HashMap<u64, (u64, String)>,
}

impl Tree for Fake {
    fn dir_path(&self, ino: u64) -> Result<Option<String>> {
        Ok(self.dirs.get(&ino).cloned())
    }
    fn children(&self, dir: u64) -> Result<Vec<Child>> {
        Ok(self.kids.get(&dir).cloned().unwrap_or_default())
    }
    fn inode(&self, ino: u64) -> Result<Option<Inode>> {
        Ok(self.inodes.get(&ino).cloned())
    }
    fn parent_ref(&self, ino: u64) -> Result<Option<(u64, String)>> {
        Ok(self.refs.get(&ino).cloned())
    }
}

fn inode(ino: u64, size: u64, kind: FileKind) -> Inode {
    Inode { ino, size, disk: size, mode: 0, mtime: 5, kind }
}

fn child(name: &str, ino: u64, kind: FileKind) -> Child {
    Child { name: name.into(), ino, kind }
}

fn indexed(path: &str, ino: u64, kind: FileKind) -> FileRecord {
    record(path.into(), &inode(ino, 1, kind), 0, &Mounted { prefix: "", fs: &FsKind::Btrfs, source: 0 })
}

fn build(name: &str) -> (std::path::PathBuf, CompactIndex) {
    let records = vec![
        indexed("/d", 300, FileKind::Dir),
        indexed("/d/keep.txt", 301, FileKind::File),
        indexed("/d/gone.txt", 302, FileKind::File),
        indexed("/d/sub", 303, FileKind::Dir),
        indexed("/d/sub/deep.txt", 304, FileKind::File),
        indexed("/old", 310, FileKind::Dir),
        indexed("/old/inner.txt", 311, FileKind::File),
    ];
    let path = std::env::temp_dir().join(format!("neutra-sweep-{name}-{}.idx", std::process::id()));
    let mut spill = neutra_core::SpillAccumulator::begin(&path).unwrap();
    spill.push_batch(records).unwrap();
    CompactIndex::rebuild_streamed(spill.finish().unwrap(), &path).unwrap();
    (path.clone(), CompactIndex::open_fast(&path).unwrap())
}

fn summary(changes: Vec<DeltaChange>) -> BTreeMap<String, String> {
    changes
        .into_iter()
        .map(|change| match change {
            DeltaChange::Remove(path) => (path.to_string(), "remove".to_string()),
            DeltaChange::Upsert(record) => (record.path.to_string(), format!("upsert {}", record.size)),
        })
        .collect()
}

#[test]
fn deletes_renames_new_files_and_edits_reach_the_index() {
    let (path, base) = build("plan");
    let mut tree = Fake::default();
    tree.dirs.extend([(256, String::new()), (300, "d".into()), (310, "moved".into())]);
    tree.kids.insert(256, vec![child("d", 300, FileKind::Dir), child("moved", 310, FileKind::Dir)]);
    tree.kids.insert(
        300,
        vec![child("keep.txt", 301, FileKind::File), child("new.txt", 305, FileKind::File)],
    );
    tree.kids.insert(310, vec![child("inner.txt", 311, FileKind::File)]);
    tree.inodes.insert(305, inode(305, 7, FileKind::File));
    tree.inodes.insert(310, inode(310, 0, FileKind::Dir));
    let changes = Changes {
        generation: 9,
        inodes: vec![
            (inode(301, 9, FileKind::File), Some((300, "keep.txt".into()))),
            (inode(305, 7, FileKind::File), Some((300, "new.txt".into()))),
        ],
        dirs: vec![256, 300],
    };
    let mounted = Mounted { prefix: "", fs: &FsKind::Btrfs, source: 0 };
    let got = summary(reconcile(&tree, &changes, &base, None, &mounted, &mut Churn::default()).unwrap());
    let want: BTreeMap<String, String> = [
        ("/d/gone.txt", "remove"),
        ("/d/sub", "remove"),
        ("/d/sub/deep.txt", "remove"),
        ("/d/new.txt", "upsert 7"),
        ("/d/keep.txt", "upsert 9"),
        ("/old", "remove"),
        ("/old/inner.txt", "remove"),
        ("/moved", "upsert 0"),
        ("/moved/inner.txt", "upsert 1"),
    ]
    .into_iter()
    .map(|(path, what)| (path.to_string(), what.to_string()))
    .collect();
    assert_eq!(got, want);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn an_unchanged_inode_and_noisy_or_nested_entries_do_nothing() {
    let (path, base) = build("quiet");
    let mut tree = Fake::default();
    tree.dirs.extend([(300, "d".into())]);
    tree.kids.insert(
        300,
        vec![
            child("keep.txt", 301, FileKind::File),
            child("gone.txt", 302, FileKind::File),
            child("sub", 303, FileKind::Dir),
            child("nested", 0, FileKind::Dir),
        ],
    );
    let changes = Changes {
        generation: 3,
        inodes: vec![(inode(301, 1, FileKind::File), Some((300, "keep.txt".into())))],
        dirs: vec![300],
    };
    let mounted = Mounted { prefix: "", fs: &FsKind::Btrfs, source: 0 };
    assert!(reconcile(&tree, &changes, &base, None, &mounted, &mut Churn::default()).unwrap().is_empty());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_directory_busy_in_most_sweeps_is_skipped_until_it_settles() {
    let mut churn = Churn::default();
    let mut allowed = Vec::new();
    for _ in 0..7 {
        churn.begin_sweep();
        allowed.push(churn.allow(300));
    }
    assert_eq!(allowed, [true, true, true, true, false, false, false]);
    for _ in 0..8 {
        churn.begin_sweep();
    }
    assert!(churn.allow(300), "history ages out once the directory goes quiet");
}


#[test]
fn a_folder_emptied_of_its_last_entry_is_found_through_its_own_inode() {
    let (path, base) = build("emptied");
    let mut tree = Fake::default();
    tree.dirs.insert(300, "d".into());
    tree.kids.insert(300, vec![]);
    let changes = Changes {
        generation: 4,
        inodes: vec![(inode(300, 0, FileKind::Dir), Some((256, "d".into())))],
        dirs: vec![],
    };
    let mounted = Mounted { prefix: "", fs: &FsKind::Btrfs, source: 0 };
    let got = summary(reconcile(&tree, &changes, &base, None, &mounted, &mut Churn::default()).unwrap());
    assert_eq!(got.get("/d/keep.txt").map(String::as_str), Some("remove"));
    assert_eq!(got.get("/d/sub/deep.txt").map(String::as_str), Some("remove"));
    let _ = std::fs::remove_file(&path);
}
