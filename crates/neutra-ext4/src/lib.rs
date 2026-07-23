//! ext2/3/4 lane using libext2fs directly on a block device or image.
//!
//! This performs an inode-table scan and directory-block iteration through
//! libext2fs (the same metadata access layer used by e2fsck). It never calls
//! read_dir/stat on the mounted namespace.

use anyhow::Result;
use neutra_core::{FileRecord, MountInfo, ScanStats};
use std::path::Path;

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use anyhow::Context;
    use neutra_core::{FileKind, FsKind};
    use std::collections::{HashMap, HashSet, VecDeque};
    use std::ffi::{CStr, CString};
    use std::os::raw::{c_char, c_int, c_long, c_uint, c_void};
    use std::time::Instant;

    type Ext2Filsys = *mut c_void;
    type InodeScan = *mut c_void;
    type ErrCode = c_long;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Ext2Inode {
        mode: u16,
        uid: u16,
        size: u32,
        atime: u32,
        ctime: u32,
        mtime: u32,
        dtime: u32,
        gid: u16,
        links: u16,
        blocks: u32,
        flags: u32,
        osd1: u32,
        block: [u32; 15],
        generation: u32,
        file_acl: u32,
        size_high: u32,
        faddr: u32,
        osd2: [u8; 12],
    }
    impl Default for Ext2Inode {
        fn default() -> Self {
            unsafe { std::mem::zeroed() }
        }
    }

    #[repr(C)]
    struct DirEntry {
        inode: u32,
        rec_len: u16,
        name_len: u16,
        name: [u8; 255],
    }

    type DirCallback = unsafe extern "C" fn(
        u32,
        c_int,
        *mut DirEntry,
        c_int,
        c_int,
        *mut c_char,
        *mut c_void,
    ) -> c_int;

    #[link(name = "ext2fs")]
    #[link(name = "com_err")]
    unsafe extern "C" {
        static unix_io_manager: *mut c_void;
        fn ext2fs_open(
            name: *const c_char,
            flags: c_int,
            superblock: c_int,
            block_size: c_uint,
            manager: *mut c_void,
            ret: *mut Ext2Filsys,
        ) -> ErrCode;
        fn ext2fs_close(fs: Ext2Filsys) -> ErrCode;
        fn ext2fs_open_inode_scan(
            fs: Ext2Filsys,
            buffer_blocks: c_int,
            ret: *mut InodeScan,
        ) -> ErrCode;
        fn ext2fs_get_next_inode(scan: InodeScan, ino: *mut u32, inode: *mut Ext2Inode) -> ErrCode;
        fn ext2fs_close_inode_scan(scan: InodeScan);
        fn ext2fs_dir_iterate2(
            fs: Ext2Filsys,
            dir: u32,
            flags: c_int,
            block_buf: *mut c_char,
            cb: DirCallback,
            data: *mut c_void,
        ) -> ErrCode;
        fn error_message(code: ErrCode) -> *const c_char;
    }

    #[derive(Clone, Copy)]
    struct Meta {
        mode: u16,
        size: u64,
        mtime: i64,
    }
    struct Fs(Ext2Filsys);
    impl Drop for Fs {
        fn drop(&mut self) {
            unsafe {
                let _ = ext2fs_close(self.0);
            }
        }
    }
    struct Scan(InodeScan);
    impl Drop for Scan {
        fn drop(&mut self) {
            unsafe {
                ext2fs_close_inode_scan(self.0);
            }
        }
    }

    fn err(code: ErrCode, op: &str) -> anyhow::Error {
        let msg = unsafe {
            let p = error_message(code);
            if p.is_null() {
                "unknown libext2fs error".into()
            } else {
                CStr::from_ptr(p).to_string_lossy().into_owned()
            }
        };
        anyhow::anyhow!("{op}: {msg} (libext2fs code {code})")
    }

    pub fn scan_image(
        path: &Path,
        prefix: &str,
        sink: &mut dyn FnMut(FileRecord),
    ) -> Result<ScanStats> {
        let started = Instant::now();
        let cpath =
            CString::new(path.as_os_str().as_encoded_bytes()).context("NUL in device path")?;
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
            let mut links = Vec::<(u32, String)>::new();
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
            children.insert(dir, links);
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

    unsafe extern "C" fn dir_cb(
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
            return 0;
        }
        let name = String::from_utf8_lossy(&d.name[..n]).into_owned();
        if name == "." || name == ".." {
            return 0;
        }
        unsafe {
            (&mut *(data as *mut Vec<(u32, String)>)).push((d.inode, name));
        }
        0
    }
    fn kind(mode: u16) -> FileKind {
        match mode & 0o170000 {
            0o100000 => FileKind::File,
            0o040000 => FileKind::Dir,
            0o120000 => FileKind::Symlink,
            _ => FileKind::Other,
        }
    }

    pub fn scan(mount: &MountInfo, sink: &mut dyn FnMut(FileRecord)) -> Result<ScanStats> {
        scan_image(
            Path::new(&mount.device),
            &mount.mountpoint.to_string_lossy(),
            sink,
        )
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn inode_layout() {
            assert_eq!(std::mem::size_of::<Ext2Inode>(), 128);
            assert_eq!(std::mem::size_of::<DirEntry>(), 264);
        }
        #[test]
        fn modes() {
            assert_eq!(kind(0o100644), FileKind::File);
            assert_eq!(kind(0o040755), FileKind::Dir);
        }
        #[test]
        fn scans_real_ext4_image_without_mounting() {
            if std::process::Command::new("mke2fs")
                .arg("-V")
                .output()
                .is_err()
            {
                return;
            }
            let base =
                std::env::temp_dir().join(format!("neutra-ext4-test-{}", std::process::id()));
            let root = base.join("root");
            let image = base.join("fixture.ext4");
            let _ = std::fs::remove_dir_all(&base);
            std::fs::create_dir_all(root.join("nested")).unwrap();
            std::fs::write(root.join("hello.txt"), b"hello").unwrap();
            std::fs::write(root.join("nested/code.rs"), b"fn main() {}\n").unwrap();
            let f = std::fs::File::create(&image).unwrap();
            f.set_len(16 * 1024 * 1024).unwrap();
            drop(f);
            let status = std::process::Command::new("mke2fs")
                .args(["-q", "-t", "ext4", "-d"])
                .arg(&root)
                .arg(&image)
                .status()
                .unwrap();
            assert!(status.success());
            let mut records = Vec::new();
            let stats = scan_image(&image, "/fixture", &mut |r| records.push(r)).unwrap();
            assert!(stats.records >= 3);
            assert!(records
                .iter()
                .any(|r| r.path.as_ref() == "/fixture/hello.txt" && r.size == 5));
            assert!(records
                .iter()
                .any(|r| r.path.as_ref() == "/fixture/nested/code.rs" && r.kind == FileKind::File));
            let _ = std::fs::remove_dir_all(base);
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::{scan, scan_image};
#[cfg(not(target_os = "linux"))]
pub fn scan(_mount: &MountInfo, _sink: &mut dyn FnMut(FileRecord)) -> Result<ScanStats> {
    anyhow::bail!("libext2fs lane is available on Linux builds")
}
#[cfg(not(target_os = "linux"))]
pub fn scan_image(
    _path: &Path,
    _prefix: &str,
    _sink: &mut dyn FnMut(FileRecord),
) -> Result<ScanStats> {
    anyhow::bail!("libext2fs lane is available on Linux builds")
}
