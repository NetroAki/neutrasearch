use super::*;

struct Root;
impl Tree for Root {
    fn dir_path(&self, _: u64) -> Result<Option<String>> {
        Ok(Some(String::new()))
    }
    fn children(&self, _: u64) -> Result<Vec<Child>> {
        Ok(vec![Child {
            name: "keep.txt".into(),
            ino: 300,
            kind: FileKind::File,
        }])
    }
    fn inode(&self, _: u64) -> Result<Option<Inode>> {
        Ok(None)
    }
    fn parent_refs(&self, _: u64) -> Result<Vec<(u64, String)>> {
        Ok(vec![(256, "..".into())])
    }
}

#[test]
fn obsolete_root_backreference_is_removed_without_stopping_live_updates() {
    let root = std::env::temp_dir().join(format!("neutra-root-ref-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let path = root.join("base.nsx");
    let mounted = Mounted {
        prefix: "/mount",
        relative_root: "",
        fs: &FsKind::Btrfs,
        source: 0,
    };
    let inode = Inode {
        ino: 256,
        size: 0,
        disk: 0,
        mode: 0,
        mtime: 5,
        kind: FileKind::Dir,
    };
    let file = Inode {
        ino: 300,
        kind: FileKind::File,
        ..inode.clone()
    };
    let mut spill = neutra_core::SpillAccumulator::begin(&path).unwrap();
    spill
        .push_batch(vec![record("/mount/keep.txt".into(), &file, 256, &mounted)])
        .unwrap();
    CompactIndex::rebuild_streamed(spill.finish().unwrap(), &path).unwrap();
    let base = CompactIndex::open_fast(&path).unwrap();
    let mut delta = DeltaIndex::open(&path.with_extension("delta"), base.generation()).unwrap();
    delta
        .apply(DeltaChange::Upsert(record(
            "/mount/..".into(),
            &inode,
            256,
            &mounted,
        )))
        .unwrap();
    let changes = Changes {
        generation: 1,
        dirs: vec![256],
        inodes: vec![(inode, Some((256, "..".into())))],
    };
    let planned = reconcile(&Root, &changes, &base, Some(&delta), &mounted, None).unwrap();
    assert!(planned
        .iter()
        .any(|c| matches!(c, DeltaChange::Remove(p) if &**p == "/mount/..")));
    assert!(!planned
        .iter()
        .any(|c| matches!(c, DeltaChange::Upsert(r) if r.path.ends_with("/.."))));
    delta.apply_batch(planned).unwrap();
    assert!(delta.is_removed("/mount/..").unwrap());
    assert!(!delta.is_removed("/mount/keep.txt").unwrap());
    drop(delta);
    drop(base);
    std::fs::remove_dir_all(root).unwrap();
}
