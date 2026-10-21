use super::domain::{DirCallback, ErrCode, Ext2Filsys, Ext2Inode, InodeScan};
use std::ffi::CStr;
use std::os::raw::{c_char, c_int, c_uint, c_void};

#[link(name = "ext2fs")]
#[link(name = "com_err")]
unsafe extern "C" {
    pub(super) static unix_io_manager: *mut c_void;
    pub(super) fn ext2fs_open(
        name: *const c_char,
        flags: c_int,
        superblock: c_int,
        block_size: c_uint,
        manager: *mut c_void,
        ret: *mut Ext2Filsys,
    ) -> ErrCode;
    pub(super) fn ext2fs_close(fs: Ext2Filsys) -> ErrCode;
    pub(super) fn ext2fs_open_inode_scan(
        fs: Ext2Filsys,
        buffer_blocks: c_int,
        ret: *mut InodeScan,
    ) -> ErrCode;
    pub(super) fn ext2fs_get_next_inode(
        scan: InodeScan,
        ino: *mut u32,
        inode: *mut Ext2Inode,
    ) -> ErrCode;
    pub(super) fn ext2fs_close_inode_scan(scan: InodeScan);
    /// Allocated size in 512-byte units, including the high bits and huge-file inodes.
    pub(super) fn ext2fs_get_stat_i_blocks(fs: Ext2Filsys, inode: *mut Ext2Inode) -> u64;
    pub(super) fn ext2fs_dir_iterate2(
        fs: Ext2Filsys,
        dir: u32,
        flags: c_int,
        block_buf: *mut c_char,
        cb: DirCallback,
        data: *mut c_void,
    ) -> ErrCode;
    pub(super) fn error_message(code: ErrCode) -> *const c_char;
}

pub(super) fn err(code: ErrCode, op: &str) -> anyhow::Error {
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
