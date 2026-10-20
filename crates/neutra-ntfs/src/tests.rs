use super::*;
use crate::records::append_component;
#[test]
fn root_parent_sequence_uses_the_preseeded_root_path() {
    let entries = HashMap::from([(
        6,
        Entry {
            parent: 5,
            parent_sequence: 42,
            sequence: 1,
            name: "Windows".into(),
            size: 0,
            alloc: 0,
            mtime: 0,
            dir: true,
        },
    )]);
    let mut cache = HashMap::from([(5, String::new())]);
    assert!(ensure_dir_path(5, 42, &entries, &mut cache));
    assert_eq!(cache.get(&5).map(String::as_str), Some(""));
}

#[test]
fn path_cycles_stop() {
    let mut e = HashMap::new();
    e.insert(
        6,
        Entry {
            parent: 5,
            parent_sequence: 0,
            sequence: 1,
            name: "a".into(),
            size: 0,
            alloc: 0,
            mtime: 0,
            dir: true,
        },
    );
    e.insert(
        7,
        Entry {
            parent: 6,
            parent_sequence: 1,
            sequence: 1,
            name: "b".into(),
            size: 0,
            alloc: 0,
            mtime: 0,
            dir: false,
        },
    );
    let mut c = HashMap::from([(5, String::new())]);
    assert!(ensure_dir_path(6, 1, &e, &mut c));
    assert_eq!(append_component(c.get(&6).unwrap(), "b"), "a/b");
    e.get_mut(&6).unwrap().parent = 7;
    let mut c = HashMap::from([(5, String::new())]);
    assert!(!ensure_dir_path(6, 1, &e, &mut c));
}

#[test]
fn scan_hard_fails_when_the_volume_will_not_open() {
    // Never walk as a fallback: an unreadable volume is a setup bug,
    // not a slow path.
    let mount = MountInfo {
        device: "/nonexistent-volume".into(),
        mountpoint: std::env::temp_dir(),
        fs: FsKind::Ntfs,
        source: neutra_core::MountSource::Local,
    };
    let mut records = Vec::new();
    let error = scan(&mount, &mut |record| records.push(record))
        .unwrap_err()
        .to_string();
    assert!(error.contains("privileges"), "unexpected: {error}");
    assert!(records.is_empty());
}
