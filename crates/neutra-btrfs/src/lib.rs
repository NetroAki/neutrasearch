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
    use std::collections::{HashMap, HashSet};
    use std::fs::File;
    use std::os::fd::AsRawFd;
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

    fn env_present(current: &str, legacy: &str) -> bool {
        std::env::var_os(current).is_some() || std::env::var_os(legacy).is_some()
    }
    fn env_value(current: &str, legacy: &str) -> Option<String> {
        std::env::var(current)
            .ok()
            .or_else(|| std::env::var(legacy).ok())
    }

    pub fn scan(mount: &MountInfo, sink: &mut dyn FnMut(FileRecord)) -> Result<ScanStats> {
        let started = Instant::now();
        let force_serial = env_present("NEUTRASEARCH_BTRFS_SERIAL", "NEUTRA_BTRFS_SERIAL")
            || env_present(
                "NEUTRASEARCH_BTRFS_MIN_OBJECTID",
                "NEUTRA_BTRFS_MIN_OBJECTID",
            );
        let (nodes, names, batches) = if force_serial {
            let file = File::open(&mount.mountpoint).with_context(|| {
                format!(
                    "open Btrfs mount {} (run 'neutrasearch index' with the required privileges)",
                    mount.mountpoint.display()
                )
            })?;
            let mut nodes = Vec::<Node>::with_capacity(13_000_000);
            let mut names = Vec::<u8>::with_capacity(256 * 1024 * 1024);
            let mut current_ino: Option<u64> = None;
            let mut current_meta: Option<Meta> = None;
            let mut current_link: Option<Link> = None;
            let range_min = env_value(
                "NEUTRASEARCH_BTRFS_MIN_OBJECTID",
                "NEUTRA_BTRFS_MIN_OBJECTID",
            )
            .and_then(|value| value.parse().ok())
            .unwrap_or(0u64);
            let range_max = env_value(
                "NEUTRASEARCH_BTRFS_MAX_OBJECTID",
                "NEUTRA_BTRFS_MAX_OBJECTID",
            )
            .and_then(|value| value.parse().ok())
            .unwrap_or(u64::MAX);
            let mut cursor = (range_min, INODE_ITEM, 0u64);
            let mut batches = 0u64;
            // Allocate/zero the large userspace result area once. Reallocating it
            // per ioctl dominated multi-million-entry scans.
            let mut search_storage = vec![0u64; SEARCH_V2_HEADER / 8 + SEARCH_BUF_U64S];
            // SAFETY: Vec<u64> is 8-byte aligned and large enough for the fixed
            // header followed immediately by buf_size bytes.
            let args = unsafe { &mut *(search_storage.as_mut_ptr().cast::<SearchArgsV2>()) };

            loop {
                args.key = SearchKey {
                    tree_id: 0,
                    min_objectid: cursor.0,
                    max_objectid: range_max,
                    min_offset: cursor.2,
                    max_offset: u64::MAX,
                    min_transid: 0,
                    max_transid: u64::MAX,
                    min_type: cursor.1,
                    max_type: INODE_EXTREF,
                    nr_items: 65_535,
                    ..SearchKey::default()
                };
                args.buf_size = (SEARCH_BUF_U64S * 8) as u64;
                // SAFETY: the fixed prefix mirrors btrfs_ioctl_search_args_v2;
                // the aligned trailing array is exactly buf_size bytes.
                let rc = unsafe {
                    libc::ioctl(file.as_raw_fd(), TREE_SEARCH_V2, args as *mut SearchArgsV2)
                };
                if rc < 0 {
                    let e = std::io::Error::last_os_error();
                    if e.raw_os_error() == Some(libc::EPERM)
                        || e.raw_os_error() == Some(libc::EACCES)
                    {
                        bail!(
                            "BTRFS_IOC_TREE_SEARCH denied; run 'neutrasearch index' as root/CAP_SYS_ADMIN"
                        );
                    }
                    return Err(e).context("BTRFS_IOC_TREE_SEARCH");
                }
                let count = args.key.nr_items as usize;
                if count == 0 {
                    break;
                }
                batches += 1;
                if env_present("NEUTRASEARCH_PROGRESS", "NEUTRA_PROGRESS")
                    && batches.is_multiple_of(25)
                {
                    eprintln!(
                        "btrfs batch={batches} count={count} cursor={:?} nodes={} wall_ms={}",
                        cursor,
                        nodes.len(),
                        started.elapsed().as_millis()
                    );
                }
                let bytes = unsafe {
                    std::slice::from_raw_parts(
                        (args as *const SearchArgsV2)
                            .cast::<u8>()
                            .add(SEARCH_V2_HEADER),
                        SEARCH_BUF_U64S * 8,
                    )
                };
                let mut pos = 0usize;
                let mut last = None;
                for _ in 0..count {
                    if pos + 32 > bytes.len() {
                        bail!("kernel returned truncated Btrfs search header");
                    }
                    let h = Header {
                        objectid: le64(&bytes[pos + 8..]),
                        offset: le64(&bytes[pos + 16..]),
                        item_type: le32(&bytes[pos + 24..]),
                        len: le32(&bytes[pos + 28..]),
                    };
                    pos += 32;
                    let end = pos
                        .checked_add(h.len as usize)
                        .context("Btrfs item length overflow")?;
                    if end > bytes.len() {
                        bail!("kernel returned truncated Btrfs search item");
                    }
                    let data = &bytes[pos..end];
                    if current_ino != Some(h.objectid) {
                        if let Some(old) = current_ino {
                            finish_node(&mut nodes, old, &mut current_meta, &mut current_link);
                        }
                        current_ino = Some(h.objectid);
                    }
                    match h.item_type {
                        INODE_ITEM if data.len() >= 148 => {
                            current_meta = Some(Meta {
                                size: le64(&data[16..]),
                                mode: le32(&data[52..]),
                                mtime: le64(&data[136..]) as i64,
                            })
                        }
                        INODE_REF if current_link.is_none() => {
                            current_link = parse_inode_ref(h.offset, data, &mut names)
                        }
                        INODE_EXTREF if current_link.is_none() => {
                            current_link = parse_inode_extref(data, &mut names)
                        }
                        _ => {}
                    }
                    pos = end;
                    last = Some((h.objectid, h.item_type, h.offset));
                }
                let Some(last) = last else { break };
                let Some(next) = next_key(last) else { break };
                if next <= cursor {
                    bail!("Btrfs search cursor did not advance");
                }
                cursor = next;
            }

            if let Some(old) = current_ino {
                finish_node(&mut nodes, old, &mut current_meta, &mut current_link);
            }
            if env_present("NEUTRASEARCH_PROGRESS", "NEUTRA_PROGRESS") {
                eprintln!(
                    "btrfs metadata phase: nodes={} batches={} wall_ms={}",
                    nodes.len(),
                    batches,
                    started.elapsed().as_millis()
                );
            }
            (nodes, names, batches)
        } else {
            parallel::scan_metadata(&mount.mountpoint, started)?
        };
        let mut stats = ScanStats::default();
        let prefix = mount
            .mountpoint
            .to_string_lossy()
            .trim_end_matches('/')
            .to_string();
        let mut dir_paths = HashMap::<u64, String>::new();
        dir_paths.insert(ROOT_INO, String::new());
        for node in &nodes {
            let meta = node.meta;
            let kind = kind_from_mode(meta.mode);
            let path = if node.ino == ROOT_INO {
                prefix.clone()
            } else {
                let parent = node.parent;
                if parent == u64::MAX || !ensure_dir_path(parent, &nodes, &names, &mut dir_paths) {
                    continue;
                }
                let parent_path = dir_paths.get(&parent).unwrap();
                let name = node_name(node, &names);
                if kind == FileKind::Dir {
                    let relative = append_component(parent_path, name);
                    let full = prefix_path(&prefix, &relative);
                    dir_paths.insert(node.ino, relative);
                    full
                } else {
                    let mut full =
                        String::with_capacity(prefix.len() + parent_path.len() + name.len() + 2);
                    full.push_str(&prefix);
                    full.push('/');
                    if !parent_path.is_empty() {
                        full.push_str(parent_path);
                        full.push('/');
                    }
                    full.push_str(name);
                    full
                }
            };
            if kind == FileKind::Dir {
                stats.dirs += 1
            } else {
                stats.files += 1
            };
            sink(FileRecord {
                path: path.into_boxed_str(),
                size: meta.size,
                mtime: meta.mtime,
                mode: meta.mode,
                kind,
                fs: FsKind::Btrfs,
                native_id: node.ino,
                native_parent: if node.parent == u64::MAX {
                    0
                } else {
                    node.parent
                },
                source: 0,
            });
            stats.records += 1;
        }
        stats.wall_ms = started.elapsed().as_millis() as u64;
        stats.detail = format!(
            "TREE_SEARCH only; {} batches; mounted subvolume tree",
            batches
        );
        Ok(stats)
    }

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
    mod tests {
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
        fn paths_resolve_and_cycles_stop() {
            let meta = Meta {
                size: 0,
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
    }
}

#[cfg(target_os = "linux")]
pub use linux::scan;

#[cfg(not(target_os = "linux"))]
pub fn scan(_mount: &MountInfo, _sink: &mut dyn FnMut(FileRecord)) -> Result<ScanStats> {
    bail!("Btrfs lane is only available on Linux")
}
