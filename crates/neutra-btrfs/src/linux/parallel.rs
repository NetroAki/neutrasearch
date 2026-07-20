use super::*;
use std::fs::File;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::Instant;

struct Shard {
    nodes: Vec<Node>,
    names: Vec<u8>,
    batches: u64,
}

pub(super) fn scan_metadata(mount: &Path, started: Instant) -> Result<(Vec<Node>, Vec<u8>, u64)> {
    const WIDTH: u64 = 8_000_000;
    const REGULAR_SHARDS: u64 = 16;
    let mut handles = Vec::with_capacity(REGULAR_SHARDS as usize + 1);
    for shard in 0..REGULAR_SHARDS {
        let path = mount.to_path_buf();
        let min = shard * WIDTH;
        let max = (shard + 1) * WIDTH - 1;
        handles.push(std::thread::spawn(move || {
            scan_range(path, min, max, started)
        }));
    }
    let path = mount.to_path_buf();
    handles.push(std::thread::spawn(move || {
        scan_range(path, REGULAR_SHARDS * WIDTH, u64::MAX, started)
    }));
    let mut parts = Vec::with_capacity(handles.len());
    for handle in handles {
        parts.push(
            handle
                .join()
                .map_err(|_| anyhow::anyhow!("Btrfs metadata worker panicked"))??,
        );
    }
    let node_count = parts.iter().map(|p| p.nodes.len()).sum();
    let name_bytes = parts.iter().map(|p| p.names.len()).sum();
    let mut nodes = Vec::with_capacity(node_count);
    let mut names = Vec::with_capacity(name_bytes);
    let mut batches = 0;
    for mut part in parts {
        let base = u32::try_from(names.len()).context("Btrfs filename arena exceeds 4 GiB")?;
        for node in &mut part.nodes {
            if node.name_len != 0 {
                node.name_off = node
                    .name_off
                    .checked_add(base)
                    .context("Btrfs filename arena offset overflow")?;
            }
        }
        names.append(&mut part.names);
        nodes.append(&mut part.nodes);
        batches += part.batches;
    }
    nodes.sort_unstable_by_key(|n| n.ino);
    if env_present("NEUTRASEARCH_PROGRESS", "NEUTRA_PROGRESS") {
        eprintln!(
            "btrfs parallel metadata phase: nodes={} batches={} wall_ms={}",
            nodes.len(),
            batches,
            started.elapsed().as_millis()
        );
    }
    Ok((nodes, names, batches))
}

fn scan_range(mount: PathBuf, range_min: u64, range_max: u64, started: Instant) -> Result<Shard> {
    let file =
        File::open(&mount).with_context(|| format!("open Btrfs mount {}", mount.display()))?;
    let mut nodes = Vec::<Node>::with_capacity(1_000_000);
    let mut names = Vec::<u8>::with_capacity(24 * 1024 * 1024);
    let mut current_ino = None;
    let mut current_meta = None;
    let mut current_link = None;
    let mut cursor = (range_min, INODE_ITEM, 0u64);
    let mut batches = 0u64;
    let mut storage = vec![0u64; SEARCH_V2_HEADER / 8 + SEARCH_BUF_U64S];
    let args = unsafe { &mut *(storage.as_mut_ptr().cast::<SearchArgsV2>()) };
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
        let rc =
            unsafe { libc::ioctl(file.as_raw_fd(), TREE_SEARCH_V2, args as *mut SearchArgsV2) };
        if rc < 0 {
            let e = std::io::Error::last_os_error();
            if e.raw_os_error() == Some(libc::EPERM) || e.raw_os_error() == Some(libc::EACCES) {
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
    if env_present("NEUTRASEARCH_PROGRESS", "NEUTRA_PROGRESS") && !nodes.is_empty() {
        eprintln!(
            "btrfs shard={range_min}..={range_max} nodes={} batches={} wall_ms={}",
            nodes.len(),
            batches,
            started.elapsed().as_millis()
        );
    }
    Ok(Shard {
        nodes,
        names,
        batches,
    })
}
