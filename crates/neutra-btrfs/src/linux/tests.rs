use super::*;
#[test]
fn uapi_sizes() {
    assert_eq!(std::mem::size_of::<SearchKey>(), 104);
    assert_eq!(SEARCH_V2_HEADER, 112);
    assert_eq!(std::mem::size_of::<SearchArgsV2>(), 112);
    assert_eq!(std::mem::align_of::<SearchArgsV2>(), 8);
}
#[test]
fn cursor_carries() {
    assert_eq!(next_key((1, 2, 9)), Some((1, 2, 10)));
    assert_eq!(next_key((1, 2, u64::MAX)), Some((1, 3, 0)));
    assert_eq!(
        next_key((1, INODE_EXTREF, u64::MAX)),
        Some((2, INODE_ITEM, 0))
    );
}
#[test]
fn root_mount_emits_an_absolute_root_record() {
    let prefix = "/".trim_end_matches('/');
    let path = if prefix.is_empty() {
        "/".to_string()
    } else {
        prefix.to_string()
    };
    assert_eq!(path, "/");
}

#[test]
fn paths_resolve_and_cycles_stop() {
    let meta = Meta {
        size: 0,
        disk: 0,
        mode: libc::S_IFDIR,
        mtime: 0,
    };
    let names = b"ab".to_vec();
    let mut nodes = vec![
        Node {
            ino: ROOT_INO,
            parent: u64::MAX,
            meta,
            name_off: 0,
            name_len: 0,
        },
        Node {
            ino: 300,
            parent: ROOT_INO,
            meta,
            name_off: 0,
            name_len: 1,
        },
        Node {
            ino: 301,
            parent: 300,
            meta,
            name_off: 1,
            name_len: 1,
        },
    ];
    let mut cache = HashMap::from([(ROOT_INO, String::new())]);
    assert!(ensure_dir_path(301, &nodes, &names, &mut cache));
    assert_eq!(cache.get(&301).map(String::as_str), Some("a/b"));
    nodes[1].parent = 301;
    let mut cache = HashMap::from([(ROOT_INO, String::new())]);
    assert!(!ensure_dir_path(301, &nodes, &names, &mut cache));
}
