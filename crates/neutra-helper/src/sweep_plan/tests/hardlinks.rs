use super::*;

#[test]
fn changed_hardlinked_inode_refreshes_every_parent_name() {
    let (path, base) = build("hardlinks");
    let mut tree = Fake::default();
    tree.dirs.extend([(300, "d".into()), (320, "other".into())]);
    tree.refs.insert(
        301,
        vec![(300, "keep.txt".into()), (320, "alias.txt".into())],
    );
    let changes = Changes {
        generation: 5,
        inodes: vec![(inode(301, 8, FileKind::File), None)],
        dirs: vec![],
    };
    let mounted = Mounted {
        prefix: "",
        relative_root: "",
        fs: &FsKind::Btrfs,
        source: 0,
    };
    let got = summary(reconcile(&tree, &changes, &base, None, &mounted, None).unwrap());
    assert_eq!(got.get("/d/keep.txt").map(String::as_str), Some("upsert 8"));
    assert_eq!(
        got.get("/other/alias.txt").map(String::as_str),
        Some("upsert 8")
    );
    let _ = std::fs::remove_file(&path);
}
