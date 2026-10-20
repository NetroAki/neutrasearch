use super::*;

mod fixture;
use fixture::Fake;

fn inode(ino: u64, size: u64, kind: FileKind) -> Inode {
    Inode {
        ino,
        size,
        disk: size,
        mode: 0,
        mtime: 5,
        kind,
    }
}

fn child(name: &str, ino: u64, kind: FileKind) -> Child {
    Child {
        name: name.into(),
        ino,
        kind,
    }
}

fn indexed(path: &str, ino: u64, kind: FileKind) -> FileRecord {
    record(
        path.into(),
        &inode(ino, 1, kind),
        0,
        &Mounted {
            prefix: "",
            relative_root: "",
            fs: &FsKind::Btrfs,
            source: 0,
        },
    )
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
            DeltaChange::RemovePrefix(path) => (path.to_string(), "remove prefix".to_string()),
            DeltaChange::Upsert(record) => {
                (record.path.to_string(), format!("upsert {}", record.size))
            }
        })
        .collect()
}

#[test]
fn deletes_renames_new_files_and_edits_reach_the_index() {
    let (path, base) = build("plan");
    let mut tree = Fake::default();
    tree.dirs.extend([
        (256, String::new()),
        (300, "d".into()),
        (310, "moved".into()),
    ]);
    tree.kids.insert(
        256,
        vec![
            child("d", 300, FileKind::Dir),
            child("moved", 310, FileKind::Dir),
        ],
    );
    tree.kids.insert(
        300,
        vec![
            child("keep.txt", 301, FileKind::File),
            child("new.txt", 305, FileKind::File),
        ],
    );
    tree.kids
        .insert(310, vec![child("inner.txt", 311, FileKind::File)]);
    tree.inodes.insert(305, inode(305, 7, FileKind::File));
    tree.inodes.insert(310, inode(310, 0, FileKind::Dir));
    let changes = Changes {
        generation: 9,
        inodes: vec![
            (
                inode(301, 9, FileKind::File),
                Some((300, "keep.txt".into())),
            ),
            (inode(305, 7, FileKind::File), Some((300, "new.txt".into()))),
        ],
        dirs: vec![256, 300],
    };
    let mounted = Mounted {
        prefix: "",
        relative_root: "",
        fs: &FsKind::Btrfs,
        source: 0,
    };
    let got = summary(reconcile(&tree, &changes, &base, None, &mounted, None).unwrap());
    let want: BTreeMap<String, String> = [
        ("/d/gone.txt", "remove"),
        ("/d/sub", "remove prefix"),
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
fn removed_folders_emit_one_prefix_without_expanding_descendants() {
    let (path, base) = build("subtree-cap");
    let mut tree = Fake::default();
    tree.dirs.insert(256, String::new());
    tree.kids
        .insert(256, vec![child("old", 310, FileKind::Dir)]);
    let changes = Changes {
        generation: 20,
        dirs: vec![256],
        inodes: vec![],
    };
    let mounted = Mounted {
        prefix: "",
        relative_root: "",
        fs: &FsKind::Btrfs,
        source: 0,
    };
    let planned = reconcile(&tree, &changes, &base, None, &mounted, None).unwrap();
    assert!(planned
        .iter()
        .any(|change| matches!(change, DeltaChange::RemovePrefix(path) if &**path == "/d")));
    assert!(!planned
        .iter()
        .any(|change| matches!(change, DeltaChange::Remove(path) if path.starts_with("/d/"))));
    let _ = std::fs::remove_file(path);
}

#[test]
fn unchanged_entries_do_nothing() {
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
        inodes: vec![(
            inode(301, 1, FileKind::File),
            Some((300, "keep.txt".into())),
        )],
        dirs: vec![300],
    };
    let mounted = Mounted {
        prefix: "",
        relative_root: "",
        fs: &FsKind::Btrfs,
        source: 0,
    };
    assert!(reconcile(&tree, &changes, &base, None, &mounted, None)
        .unwrap()
        .is_empty());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn duplicate_touched_directory_is_reconciled_once() {
    let (path, base) = build("duplicate-touched");
    let mut tree = Fake::default();
    tree.dirs.insert(300, "d".into());
    tree.kids.insert(
        300,
        vec![
            child("keep.txt", 301, FileKind::File),
            child("gone.txt", 302, FileKind::File),
            child("sub", 303, FileKind::Dir),
        ],
    );
    let changes = Changes {
        generation: 4,
        dirs: vec![300, 300],
        inodes: vec![(inode(300, 1, FileKind::Dir), None)],
    };
    let mounted = Mounted {
        prefix: "",
        relative_root: "",
        fs: &FsKind::Btrfs,
        source: 0,
    };
    let got = reconcile(&tree, &changes, &base, None, &mounted, None).unwrap();
    assert!(got.is_empty());
    assert_eq!(tree.child_reads.get(), 1);
    let _ = std::fs::remove_file(path);
}

#[test]
fn repeatedly_changed_files_are_never_deferred() {
    let (path, base) = build("busy");
    let mut tree = Fake::default();
    tree.dirs.insert(300, "d".into());
    let mounted = Mounted {
        prefix: "",
        relative_root: "",
        fs: &FsKind::Btrfs,
        source: 0,
    };
    for generation in 2..20 {
        let changes = Changes {
            generation,
            inodes: vec![(
                inode(301, generation, FileKind::File),
                Some((300, "keep.txt".into())),
            )],
            dirs: vec![],
        };
        let got = summary(reconcile(&tree, &changes, &base, None, &mounted, None).unwrap());
        assert_eq!(
            got.get("/d/keep.txt"),
            Some(&format!("upsert {generation}"))
        );
    }
    let _ = std::fs::remove_file(path);
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
    let mounted = Mounted {
        prefix: "",
        relative_root: "",
        fs: &FsKind::Btrfs,
        source: 0,
    };
    let got = summary(reconcile(&tree, &changes, &base, None, &mounted, None).unwrap());
    assert_eq!(got.get("/d/keep.txt").map(String::as_str), Some("remove"));
    assert_eq!(got.get("/d/sub").map(String::as_str), Some("remove prefix"));
    let _ = std::fs::remove_file(&path);
}

mod hardlinks;
mod mount_scope;
