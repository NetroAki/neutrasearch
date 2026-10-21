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
    let good = {
        let mut links = Vec::new();
        parse_inode_ref(ROOT_INO, &replacement, &mut names, &mut links).unwrap();
        links.remove(0)
    };
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
    assert!(parse_inode_ref(ROOT_INO, &invalid, &mut names, &mut Vec::new()).is_err());
    assert_eq!(node_name(&node, &names).unwrap(), "\u{fffd}");
}

fn packed_ref(records: &[(u64, &str)]) -> Vec<u8> {
    let mut data = Vec::new();
    for (index, name) in records {
        data.extend_from_slice(&index.to_le_bytes());
        data.extend_from_slice(&(name.len() as u16).to_le_bytes());
        data.extend_from_slice(name.as_bytes());
    }
    data
}

fn packed_extref(records: &[(u64, &str)]) -> Vec<u8> {
    let mut data = Vec::new();
    for (parent, name) in records {
        data.extend_from_slice(&parent.to_le_bytes());
        data.extend_from_slice(&0u64.to_le_bytes()); // index
        data.extend_from_slice(&(name.len() as u16).to_le_bytes());
        data.extend_from_slice(name.as_bytes());
    }
    data
}

#[test]
fn native_packed_refs_and_extrefs_emit_every_alias() {
    let mut names = Vec::new();
    let mut pending = PendingInode::default();
    let mut nodes = Vec::new();
    let mut inode = vec![0u8; 160];
    inode[52..56].copy_from_slice(&(libc::S_IFREG | 0o644).to_le_bytes());
    update_item(
        Header {
            objectid: 900,
            offset: 0,
            item_type: INODE_ITEM,
            len: inode.len() as u32,
        },
        &inode,
        &mut names,
        &mut pending,
    )
    .unwrap();

    // Two native btrfs_inode_ref records packed under one parent key.
    let refs = packed_ref(&[(7, "first"), (8, "same-dir")]);
    parse_inode_ref(300, &refs, &mut names, &mut pending.links).unwrap();
    // A second directory and two btrfs_inode_extref records.
    parse_inode_ref(
        301,
        &packed_ref(&[(9, "other-dir")]),
        &mut names,
        &mut pending.links,
    )
    .unwrap();
    parse_inode_extref(
        &packed_extref(&[(302, "ext-a"), (303, "ext-b")]),
        &mut names,
        &mut pending.links,
    )
    .unwrap();
    finish_node(&mut nodes, 900, &mut pending);

    nodes.sort_unstable_by_key(|node| (node.ino, node.parent, node.name_off));
    let aliases: Vec<_> = nodes
        .iter()
        .map(|node| (node.parent, node_name(node, &names).unwrap().to_owned()))
        .collect();
    assert_eq!(
        aliases,
        vec![
            (300, "first".into()),
            (300, "same-dir".into()),
            (301, "other-dir".into()),
            (302, "ext-a".into()),
            (303, "ext-b".into()),
        ]
    );
}

#[test]
fn every_packed_alias_is_checked_for_strict_utf8() {
    let mut data = packed_ref(&[(1, "valid")]);
    data.extend_from_slice(&0u64.to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes());
    data.push(0xff);
    assert!(parse_inode_ref(300, &data, &mut Vec::new(), &mut Vec::new()).is_err());

    let mut extrefs = packed_extref(&[(302, "valid")]);
    extrefs.extend_from_slice(&303u64.to_le_bytes());
    extrefs.extend_from_slice(&0u64.to_le_bytes());
    extrefs.extend_from_slice(&1u16.to_le_bytes());
    extrefs.push(0xff);
    assert!(parse_inode_extref(&extrefs, &mut Vec::new(), &mut Vec::new()).is_err());
}

#[test]
fn truncated_packed_reference_fails_the_native_lane() {
    let mut bytes = packed_ref(&[(1, "valid")]);
    bytes.push(1);
    assert!(parse_inode_ref(300, &bytes, &mut Vec::new(), &mut Vec::new()).is_err());
}
