use super::adapters::*;
use super::domain::*;
use anyhow::{Context, Result};
use neutra_core::{FileKind, FileRecord, FsKind, MountInfo, ScanStats};
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_void};
use std::path::Path;
use std::time::Instant;

pub fn scan_image(
    path: &Path,
    prefix: &str,
    sink: &mut dyn FnMut(FileRecord),
) -> Result<ScanStats> {
    let started = Instant::now();
    let cpath = CString::new(path.as_os_str().as_encoded_bytes()).context("NUL in device path")?;
    let mut raw = std::ptr::null_mut();
    let rc = unsafe { ext2fs_open(cpath.as_ptr(), 0x20000, 0, 0, unix_io_manager, &mut raw) };
    if rc != 0 {
        return Err(err(rc, "ext2fs_open")).with_context(|| {
            format!(
                "open {} (run helper as root for block devices)",
                path.display()
            )
        });
    }
    let fs = Fs(raw);
    let mut scan_raw = std::ptr::null_mut();
    let rc = unsafe { ext2fs_open_inode_scan(fs.0, 0, &mut scan_raw) };
    if rc != 0 {
        return Err(err(rc, "ext2fs_open_inode_scan"));
    }
    let scan = Scan(scan_raw);
    let mut metas = HashMap::<u32, Meta>::new();
    let mut dirs = Vec::new();
    loop {
        let mut ino = 0u32;
        let mut inode = Ext2Inode::default();
        let rc = unsafe { ext2fs_get_next_inode(scan.0, &mut ino, &mut inode) };
        if rc != 0 {
            return Err(err(rc, "ext2fs_get_next_inode"));
        }
        if ino == 0 {
            break;
        }
        if inode.mode == 0 || inode.links == 0 {
            continue;
        }
        let kind = kind(inode.mode);
        let disk = (inode.blocks as u64).saturating_mul(512);
        let size = if kind == FileKind::File {
            inode.size as u64 | ((inode.size_high as u64) << 32)
        } else {
            inode.size as u64
        };
        metas.insert(
            ino,
            Meta {
                mode: inode.mode,
                size,
                disk,
                mtime: inode.mtime as i64,
            },
        );
        if kind == FileKind::Dir {
            dirs.push(ino);
        }
    }
    drop(scan);

    let mut children = HashMap::<u32, Vec<(u32, String)>>::new();
    for dir in dirs {
        let mut links = DirIterData {
            links: Vec::new(),
            error: None,
        };
        let rc = unsafe {
            ext2fs_dir_iterate2(
                fs.0,
                dir,
                0,
                std::ptr::null_mut(),
                dir_cb,
                &mut links as *mut _ as *mut c_void,
            )
        };
        if rc != 0 {
            return Err(err(rc, "ext2fs_dir_iterate2"));
        }
        if let Some(error) = links.error {
            return Err(error);
        }
        children.insert(dir, links.links);
    }

    let mut stats = ScanStats::default();
    let root = prefix.trim_end_matches('/');
    let mut q = VecDeque::from([(2u32, root.to_string())]);
    let mut expanded = HashSet::new();
    while let Some((parent, parent_path)) = q.pop_front() {
        if !expanded.insert(parent) {
            continue;
        }
        let Some(kids) = children.get(&parent) else {
            continue;
        };
        for (ino, name) in kids {
            let Some(meta) = metas.get(ino) else { continue };
            let path = format!("{parent_path}/{name}");
            let fk = kind(meta.mode);
            if fk == FileKind::Dir {
                stats.dirs += 1;
                q.push_back((*ino, path.clone()));
            } else {
                stats.files += 1;
            }
            sink(FileRecord {
                path: path.into_boxed_str(),
                size: meta.size,
                disk: FileRecord::allocated_bytes(meta.disk),
                mtime: meta.mtime,
                mode: meta.mode as u32,
                kind: fk,
                fs: FsKind::Ext4,
                native_id: *ino as u64,
                native_parent: parent as u64,
                source: 0,
            });
            stats.records += 1;
        }
    }
    stats.wall_ms = started.elapsed().as_millis() as u64;
    stats.detail = format!(
        "libext2fs inode table + directory blocks; {} allocated inodes",
        metas.len()
    );
    Ok(stats)
}

pub(super) unsafe extern "C" fn dir_cb(
    _dir: u32,
    entry_kind: c_int,
    de: *mut DirEntry,
    _off: c_int,
    _bs: c_int,
    _buf: *mut c_char,
    data: *mut c_void,
) -> c_int {
    if de.is_null() || data.is_null() || entry_kind != 3 {
        return 0;
    } // DIRENT_OTHER_FILE
      // SAFETY: libext2fs owns `de` for this synchronous callback and data
      // points to the Vec passed directly to ext2fs_dir_iterate2.
    let d = unsafe { &*de };
    if d.inode == 0 {
        return 0;
    }
    let n = (d.name_len & 0xff) as usize;
    if n == 0 || n > 255 {
        if let Some(out) = unsafe { (data as *mut DirIterData).as_mut() } {
            out.error = Some(anyhow::anyhow!("invalid ext4 filename length {n}"));
        }
        return 0;
    }
    let name = match std::str::from_utf8(&d.name[..n]) {
        Ok(name) => name.to_owned(),
        Err(error) => {
            if let Some(out) = unsafe { (data as *mut DirIterData).as_mut() } {
                out.error = Some(anyhow::anyhow!("ext4 filename is not UTF-8: {error}"));
            }
            return 0;
        }
    };
    if name == "." || name == ".." {
        return 0;
    }
    unsafe {
        (&mut *(data as *mut DirIterData))
            .links
            .push((d.inode, name));
    }
    0
}
pub fn scan(mount: &MountInfo, sink: &mut dyn FnMut(FileRecord)) -> Result<ScanStats> {
    scan_image(
        Path::new(&mount.device),
        &mount.mountpoint.to_string_lossy(),
        sink,
    )
}
