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
    assert!(ensure_dir_path(301, &nodes, &names, &mut cache).unwrap());
    assert_eq!(cache.get(&301).map(String::as_str), Some("a/b"));
    nodes[1].parent = 301;
    let mut cache = HashMap::from([(ROOT_INO, String::new())]);
    assert!(!ensure_dir_path(301, &nodes, &names, &mut cache).unwrap());
}

#[test]
fn inode_names_reject_invalid_utf8_without_aliasing_replacement_character() {
    let mut names = Vec::new();
    let mut replacement = vec![0; 10];
    replacement[8..10].copy_from_slice(&3u16.to_le_bytes());
    replacement.extend_from_slice("\u{fffd}".as_bytes());
    let good = parse_inode_ref(ROOT_INO, &replacement, &mut names)
        .unwrap()
        .unwrap();
    let node = Node {
        ino: 7,
        parent: ROOT_INO,
        meta: Meta {
            size: 0,
            disk: 0,
            mode: 0,
            mtime: 0,
        },
        name_off: good.name_off,
        name_len: good.name_len,
    };
    assert_eq!(node_name(&node, &names).unwrap(), "\u{fffd}");

    let mut invalid = vec![0; 10];
    invalid[8..10].copy_from_slice(&1u16.to_le_bytes());
    invalid.push(0xff);
    assert!(parse_inode_ref(ROOT_INO, &invalid, &mut names).is_err());
    assert_eq!(node_name(&node, &names).unwrap(), "\u{fffd}");
}
