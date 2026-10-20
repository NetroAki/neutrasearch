use neutra_zfs::{parse_diff, DiffEntry};

#[test]
fn decodes_real_openzfs_snapshot_delta() {
    let entries = parse_diff(include_str!("fixtures/openzfs-2.2-diff.txt")).unwrap();
    assert_eq!(
        entries,
        vec![
            DiffEntry::Modified("/qa-zfs/data/modify.txt".into()),
            DiffEntry::Removed("/qa-zfs/data/remove.txt".into()),
            DiffEntry::Renamed {
                from: "/qa-zfs/data/old name.wav".into(),
                to: "/qa-zfs/data/new name.wav".into(),
            },
            DiffEntry::Modified("/qa-zfs/data/".into()),
            DiffEntry::Created("/qa-zfs/data/new image.png".into()),
            DiffEntry::Created("/qa-zfs/data/accent-é.txt".into()),
            DiffEntry::Created("/qa-zfs/data/tab\tname.txt".into()),
            DiffEntry::Created("/qa-zfs/data/back\\slash.txt".into()),
            DiffEntry::Created("/qa-zfs/data/sparse-docker.vhdx".into()),
        ]
    );
}

#[test]
fn rename_without_type_column_keeps_both_paths() {
    assert_eq!(
        parse_diff("R\t/tank/old\t/tank/new\n").unwrap(),
        vec![DiffEntry::Renamed {
            from: "/tank/old".into(),
            to: "/tank/new".into(),
        }]
    );
}

#[test]
fn malformed_delta_cannot_emit_partial_results() {
    for malformed in [
        "+\tF\t/tank/incomplete\\004",
        "+\tF\t/tank/invalid\\0080",
        "+\tF\t/tank/wide\\0400",
        "+\tF\t/tank/nul\\0000",
        "+\tF\trelative",
        "R\tF\t/tank/from",
    ] {
        assert!(parse_diff(&format!("+\tF\t/tank/valid\n{malformed}")).is_err());
    }
}

#[cfg(unix)]
#[test]
fn non_utf8_names_keep_the_original_bytes() {
    use std::os::unix::ffi::OsStrExt;
    let entries = parse_diff("+\tF\t/tank/raw\\0377").unwrap();
    let DiffEntry::Created(path) = &entries[0] else {
        panic!("expected a created path");
    };
    assert_eq!(path.as_os_str().as_bytes(), b"/tank/raw\xff");
}

#[test]
fn unavailable_dataset_or_reader_emits_nothing() {
    use neutra_core::{FsKind, MountInfo, MountSource};
    let mount = MountInfo {
        device: "neutraqa/data".into(),
        mountpoint: "/qa-zfs/data".into(),
        fs: FsKind::Zfs,
        source: MountSource::Local,
    };
    let mut emitted = 0;
    assert!(neutra_zfs::scan(&mount, &mut |_| emitted += 1).is_err());
    assert_eq!(emitted, 0);
}
