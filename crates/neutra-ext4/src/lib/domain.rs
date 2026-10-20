use super::adapters::{ext2fs_close, ext2fs_close_inode_scan};
use std::os::raw::{c_char, c_int, c_long, c_void};
pub(super) type Ext2Filsys = *mut c_void;
pub(super) type InodeScan = *mut c_void;
pub(super) type ErrCode = c_long;

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct Ext2Inode {
    pub(super) mode: u16,
    pub(super) uid: u16,
    pub(super) size: u32,
    pub(super) atime: u32,
    pub(super) ctime: u32,
    pub(super) mtime: u32,
    pub(super) dtime: u32,
    pub(super) gid: u16,
    pub(super) links: u16,
    pub(super) blocks: u32,
    pub(super) flags: u32,
    pub(super) osd1: u32,
    pub(super) block: [u32; 15],
    pub(super) generation: u32,
    pub(super) file_acl: u32,
    pub(super) size_high: u32,
    pub(super) faddr: u32,
    pub(super) osd2: [u8; 12],
}
impl Default for Ext2Inode {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

#[repr(C)]
pub(super) struct DirEntry {
    pub(super) inode: u32,
    pub(super) rec_len: u16,
    pub(super) name_len: u16,
    pub(super) name: [u8; 255],
}

pub(super) type DirCallback = unsafe extern "C" fn(
    u32,
    c_int,
    *mut DirEntry,
    c_int,
    c_int,
    *mut c_char,
    *mut c_void,
) -> c_int;

use neutra_core::FileKind;

#[derive(Clone, Copy)]
pub(super) struct Meta {
    pub(super) mode: u16,
    pub(super) size: u64,
    pub(super) disk: u64,
    pub(super) mtime: i64,
}
pub(super) struct Fs(pub(super) Ext2Filsys);
impl Drop for Fs {
    fn drop(&mut self) {
        unsafe {
            let _ = ext2fs_close(self.0);
        }
    }
}
pub(super) struct Scan(pub(super) InodeScan);
impl Drop for Scan {
    fn drop(&mut self) {
        unsafe {
            ext2fs_close_inode_scan(self.0);
        }
    }
}

pub(super) fn kind(mode: u16) -> FileKind {
    match mode & 0o170000 {
        0o100000 => FileKind::File,
        0o040000 => FileKind::Dir,
        0o120000 => FileKind::Symlink,
        _ => FileKind::Other,
    }
}
