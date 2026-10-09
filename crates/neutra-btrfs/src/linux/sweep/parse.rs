use super::super::*;
use super::{Child, Inode, Key, ROOT_ITEM};

pub(super) fn successor((object, kind, offset): Key) -> Option<Key> {
    if offset < u64::MAX {
        Some((object, kind, offset + 1))
    } else if kind < u32::MAX {
        Some((object, kind + 1, 0))
    } else {
        object.checked_add(1).map(|next| (next, 0, 0))
    }
}

pub(super) fn parse_inode(ino: u64, data: &[u8]) -> Inode {
    let mode = le32(&data[52..]);
    Inode {
        ino,
        size: le64(&data[16..]),
        disk: le64(&data[24..]),
        mode,
        mtime: le64(&data[136..]) as i64,
        kind: kind_from_mode(mode),
    }
}

pub(super) fn dir_changed(data: &[u8], since: u64) -> bool {
    // Leaf rewrites include old neighbors; the entry's own transid filters them.
    data.len() >= 30 && le64(&data[17..]) >= since
}

pub(super) fn parse_ref(parent: u64, data: &[u8]) -> Option<(u64, String)> {
    if data.len() < 10 {
        return None;
    }
    let len = u16::from_le_bytes([data[8], data[9]]) as usize;
    let name = data.get(10..10 + len)?;
    Some((parent, String::from_utf8_lossy(name).into_owned()))
}

/// Decode a btrfs_inode_extref: parent objectid, index, name length, name.
pub(super) fn parse_extref(data: &[u8]) -> Option<(u64, String)> {
    if data.len() < 18 {
        return None;
    }
    let parent = u64::from_le_bytes(data[..8].try_into().ok()?);
    let len = u16::from_le_bytes(data[16..18].try_into().ok()?) as usize;
    let name = data.get(18..18 + len)?;
    Some((parent, String::from_utf8_lossy(name).into_owned()))
}

pub(super) fn parse_refs(parent: u64, mut data: &[u8], extended: bool) -> Vec<(u64, String)> {
    let header = if extended { 18 } else { 10 };
    let mut refs = Vec::new();
    while data.len() >= header {
        let length = u16::from_le_bytes(data[header - 2..header].try_into().unwrap()) as usize;
        let Some(entry) = data.get(..header + length) else {
            break;
        };
        let parsed = if extended {
            parse_extref(entry)
        } else {
            parse_ref(parent, entry)
        };
        if let Some(link) = parsed {
            refs.push(link);
        }
        data = &data[entry.len()..];
    }
    refs
}

/// One `btrfs_dir_item`: the location key, then type and name. An entry that
/// points at another subvolume's root has no inode in this tree; it is kept
/// with inode 0 so the name still counts as present.
pub(super) fn parse_dir_index(data: &[u8]) -> Option<Child> {
    if data.len() < 30 || (data[8] != INODE_ITEM as u8 && data[8] != ROOT_ITEM as u8) {
        return None;
    }
    let name_len = u16::from_le_bytes([data[27], data[28]]) as usize;
    let name = data.get(30..30 + name_len)?;
    let kind = match data[29] {
        1 => FileKind::File,
        2 => FileKind::Dir,
        7 => FileKind::Symlink,
        _ => FileKind::Other,
    };
    let ino = if data[8] == INODE_ITEM as u8 {
        le64(data)
    } else {
        0
    };
    Some(Child {
        name: String::from_utf8_lossy(name).into_owned(),
        ino,
        kind,
    })
}

#[cfg(test)]
mod tests {
    use super::super::{InoLookupArgs, INO_LOOKUP_BYTES};
    use super::*;

    #[test]
    fn rewritten_leaves_do_not_reconcile_unchanged_directory_neighbors() {
        let mut entry = [0u8; 30];
        entry[17..25].copy_from_slice(&41u64.to_le_bytes());
        assert!(!dir_changed(&entry, 42));
        assert!(dir_changed(&entry, 41));
        entry[17..25].copy_from_slice(&43u64.to_le_bytes());
        assert!(dir_changed(&entry, 42));
        assert!(!dir_changed(&entry[..24], 42));
    }

    #[test]
    fn dir_index_items_decode_name_kind_and_target() {
        let mut data = vec![0u8; 30];
        data[..8].copy_from_slice(&300u64.to_le_bytes());
        data[8] = INODE_ITEM as u8;
        data[27..29].copy_from_slice(&5u16.to_le_bytes());
        data[29] = 2;
        data.extend_from_slice(b"hello");
        let child = parse_dir_index(&data).unwrap();
        assert_eq!(
            (child.name.as_str(), child.ino, child.kind),
            ("hello", 300, FileKind::Dir)
        );
        data[8] = ROOT_ITEM as u8;
        assert_eq!(
            parse_dir_index(&data).unwrap().ino,
            0,
            "nested subvolumes keep their name"
        );
        data[8] = 77;
        assert!(parse_dir_index(&data).is_none());
    }

    #[test]
    fn keys_advance_through_offset_type_and_object() {
        assert_eq!(successor((1, 2, 3)), Some((1, 2, 4)));
        assert_eq!(successor((1, 2, u64::MAX)), Some((1, 3, 0)));
        assert_eq!(successor((1, u32::MAX, u64::MAX)), Some((2, 0, 0)));
        assert_eq!(successor((u64::MAX, u32::MAX, u64::MAX)), None);
    }

    #[test]
    fn extref_decodes_parent_and_name_after_index() {
        let mut data = vec![0; 18];
        data[..8].copy_from_slice(&77u64.to_le_bytes());
        data[8..16].copy_from_slice(&987654u64.to_le_bytes());
        data[16..18].copy_from_slice(&4u16.to_le_bytes());
        data.extend_from_slice(b"name");
        assert_eq!(parse_extref(&data), Some((77, "name".into())));
    }

    #[test]
    fn malformed_extref_is_rejected() {
        assert_eq!(parse_extref(&[0; 17]), None);
    }

    #[test]
    fn every_name_in_a_packed_backreference_is_preserved() {
        for extended in [false, true] {
            let header = if extended { 18 } else { 10 };
            let mut data = Vec::new();
            for name in ["one", "é"] {
                let mut entry = vec![0u8; header];
                if extended {
                    entry[..8].copy_from_slice(&77u64.to_le_bytes());
                }
                entry[header - 2..].copy_from_slice(&(name.len() as u16).to_le_bytes());
                entry.extend_from_slice(name.as_bytes());
                data.extend(entry);
            }
            assert_eq!(
                parse_refs(77, &data, extended),
                vec![(77, "one".into()), (77, "é".into())]
            );
        }
    }

    #[test]
    fn ino_lookup_request_is_the_documented_4096_bytes() {
        assert_eq!(std::mem::size_of::<InoLookupArgs>(), INO_LOOKUP_BYTES);
    }
}
