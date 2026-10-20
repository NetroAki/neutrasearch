//! Btrfs metadata lane using `BTRFS_IOC_TREE_SEARCH` only.
//!
//! No `read_dir`, `stat`, or mounted-tree walk exists in this crate. Tree id
//! zero asks the kernel for the subvolume containing the opened mountpoint.

#[cfg(target_os = "linux")]
use anyhow::Context;
use anyhow::{bail, Result};
#[cfg(target_os = "linux")]
use neutra_core::{FileKind, FsKind};
use neutra_core::{FileRecord, MountInfo, ScanStats};

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    mod parallel;
    pub mod sweep;
    use std::collections::{HashMap, HashSet};
    use std::time::Instant;

    const INODE_ITEM: u32 = 1;
    const INODE_REF: u32 = 12;
    const INODE_EXTREF: u32 = 13;
    const ROOT_INO: u64 = 256;
    const SEARCH_BUF_U64S: usize = 512 * 1024; // reusable 4 MiB V2 buffer

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct SearchKey {
        tree_id: u64,
        min_objectid: u64,
        max_objectid: u64,
        min_offset: u64,
        max_offset: u64,
        min_transid: u64,
        max_transid: u64,
        min_type: u32,
        max_type: u32,
        nr_items: u32,
        unused: u32,
        unused1: u64,
        unused2: u64,
        unused3: u64,
        unused4: u64,
    }

    #[repr(C)]
    struct SearchArgsV2 {
        key: SearchKey,
        buf_size: u64,
    }

    #[derive(Clone, Copy, Debug)]
    struct Header {
        objectid: u64,
        offset: u64,
        item_type: u32,
        len: u32,
    }

    #[derive(Clone, Copy, Debug)]
    struct Meta {
        size: u64,
        disk: u64,
        mode: u32,
        mtime: i64,
    }
    #[derive(Clone, Copy, Debug)]
    struct Link {
        parent: u64,
        name_off: u32,
        name_len: u16,
    }
    #[derive(Debug)]
    struct Node {
        ino: u64,
        parent: u64,
        meta: Meta,
        name_off: u32,
        name_len: u16,
    }

    fn finish_node(
        nodes: &mut Vec<Node>,
        ino: u64,
        meta: &mut Option<Meta>,
        link: &mut Option<Link>,
    ) {
        if let Some(m) = meta.take() {
            if ino == ROOT_INO {
                nodes.push(Node {
                    ino,
                    parent: u64::MAX,
                    meta: m,
                    name_off: 0,
                    name_len: 0,
                });
            } else if let Some(l) = link.take() {
                nodes.push(Node {
                    ino,
                    parent: l.parent,
                    meta: m,
                    name_off: l.name_off,
                    name_len: l.name_len,
                });
            }
        }
        *link = None;
    }

    const IOC_WRITE: u64 = 1;
    const IOC_READ: u64 = 2;
    const IOC_NRSHIFT: u64 = 0;
    const IOC_TYPESHIFT: u64 = 8;
    const IOC_SIZESHIFT: u64 = 16;
    const IOC_DIRSHIFT: u64 = 30;
    // UAPI request size is the fixed V2 header (SearchKey + buf_size), not
    // the caller-owned flexible result buffer that follows it.
    const SEARCH_V2_HEADER: usize = std::mem::size_of::<SearchKey>() + 8;
    const TREE_SEARCH_V2: libc::c_ulong = (((IOC_READ | IOC_WRITE) << IOC_DIRSHIFT)
        | ((SEARCH_V2_HEADER as u64) << IOC_SIZESHIFT)
        | (0x94 << IOC_TYPESHIFT)
        | (17 << IOC_NRSHIFT)) as libc::c_ulong;

    mod scan;
    pub use scan::scan;

    fn store_name(parent: u64, bytes: &[u8], names: &mut Vec<u8>) -> Option<Link> {
        let text = String::from_utf8_lossy(bytes);
        let raw = text.as_bytes();
        let off = u32::try_from(names.len()).ok()?;
        let len = u16::try_from(raw.len()).ok()?;
        names.extend_from_slice(raw);
        Some(Link {
            parent,
            name_off: off,
            name_len: len,
        })
    }
    fn parse_inode_ref(parent: u64, data: &[u8], names: &mut Vec<u8>) -> Option<Link> {
        if data.len() < 10 {
            return None;
        }
        let n = u16::from_le_bytes([data[8], data[9]]) as usize;
        if data.len() < 10 + n {
            return None;
        }
        store_name(parent, &data[10..10 + n], names)
    }
    fn parse_inode_extref(data: &[u8], names: &mut Vec<u8>) -> Option<Link> {
        if data.len() < 18 {
            return None;
        }
        let parent = le64(data);
        let n = u16::from_le_bytes([data[16], data[17]]) as usize;
        if data.len() < 18 + n {
            return None;
        }
        store_name(parent, &data[18..18 + n], names)
    }
    fn node_name<'a>(node: &Node, names: &'a [u8]) -> &'a str {
        std::str::from_utf8(
            &names[node.name_off as usize..node.name_off as usize + node.name_len as usize],
        )
        .unwrap()
    }

    fn ensure_dir_path(
        ino: u64,
        nodes: &[Node],
        names: &[u8],
        cache: &mut HashMap<u64, String>,
    ) -> bool {
        if cache.contains_key(&ino) {
            return true;
        }
        let mut current = ino;
        let mut chain = Vec::<usize>::new();
        let mut seen = HashSet::new();
        while !cache.contains_key(&current) {
            let Ok(i) = nodes.binary_search_by_key(&current, |n| n.ino) else {
                return false;
            };
            if kind_from_mode(nodes[i].meta.mode) != FileKind::Dir || !seen.insert(i) {
                return false;
            }
            chain.push(i);
            let parent = nodes[i].parent;
            if parent == u64::MAX {
                return false;
            }
            current = parent;
        }
        let mut path = cache.get(&current).unwrap().clone();
        for i in chain.into_iter().rev() {
            path = append_component(&path, node_name(&nodes[i], names));
            cache.insert(nodes[i].ino, path.clone());
        }
        true
    }
    fn append_component(parent: &str, name: &str) -> String {
        let mut p = String::with_capacity(parent.len() + name.len() + 1);
        p.push_str(parent);
        if !parent.is_empty() {
            p.push('/');
        }
        p.push_str(name);
        p
    }
    fn prefix_path(prefix: &str, relative: &str) -> String {
        if relative.is_empty() {
            return prefix.to_string();
        }
        let mut p = String::with_capacity(prefix.len() + relative.len() + 1);
        p.push_str(prefix);
        p.push('/');
        p.push_str(relative);
        p
    }

    fn next_key((obj, typ, off): (u64, u32, u64)) -> Option<(u64, u32, u64)> {
        if off != u64::MAX {
            Some((obj, typ, off + 1))
        } else if typ < INODE_EXTREF {
            Some((obj, typ + 1, 0))
        } else {
            obj.checked_add(1).map(|o| (o, INODE_ITEM, 0))
        }
    }

    fn kind_from_mode(mode: u32) -> FileKind {
        match mode & libc::S_IFMT {
            libc::S_IFREG => FileKind::File,
            libc::S_IFDIR => FileKind::Dir,
            libc::S_IFLNK => FileKind::Symlink,
            _ => FileKind::Other,
        }
    }
    fn le64(b: &[u8]) -> u64 {
        u64::from_le_bytes(b[..8].try_into().unwrap())
    }
    fn le32(b: &[u8]) -> u32 {
        u32::from_le_bytes(b[..4].try_into().unwrap())
    }

    #[cfg(test)]
    mod tests;
}

#[cfg(target_os = "linux")]
pub use linux::scan;
#[cfg(target_os = "linux")]
pub use linux::sweep::{Changes, Child, Inode, Subvolume};

#[cfg(not(target_os = "linux"))]
pub fn scan(_mount: &MountInfo, _sink: &mut dyn FnMut(FileRecord)) -> Result<ScanStats> {
    bail!("Btrfs lane is only available on Linux")
}
