//! The labeled getattrlistbulk fallback used when Spotlight cannot supply a
//! volume: one bulk-attribute sweep per directory, breadth-first, never
//! readdir. Split from the Spotlight selection so the two macOS lanes stay
//! independently reviewable.

use super::{FileRecord, MountInfo, ScanStats};
use anyhow::{Context as _, Result};

pub(super) fn bulk_fallback(mount: &MountInfo, sink: &mut dyn FnMut(FileRecord)) -> Result<ScanStats> {
    use anyhow::Context as _;
    use neutra_core::FileKind;
    use std::collections::VecDeque;
    use std::ffi::{CString, OsString};
    use std::os::fd::RawFd;
    use std::os::unix::ffi::OsStringExt;
    use std::time::Instant;

    #[repr(C)]
    struct AttrList {
        bitmapcount: u16,
        reserved: u16,
        commonattr: u32,
        volattr: u32,
        dirattr: u32,
        fileattr: u32,
        forkattr: u32,
    }
    unsafe extern "C" {
        fn getattrlistbulk(
            dirfd: i32,
            attrs: *mut AttrList,
            buf: *mut libc::c_void,
            size: usize,
            options: u64,
        ) -> i32;
    }
    const ATTR_BIT_MAP_COUNT: u16 = 5;
    const ATTR_CMN_NAME: u32 = 0x0000_0001;
    const ATTR_CMN_OBJTYPE: u32 = 0x0000_0008;
    const ATTR_CMN_RETURNED_ATTRS: u32 = 0x8000_0000;
    const VDIR: u32 = 2;

    struct OwnedFd(RawFd);
    impl Drop for OwnedFd {
        fn drop(&mut self) {
            unsafe {
                libc::close(self.0);
            }
        }
    }
    let started = Instant::now();
    let root_c = CString::new(mount.mountpoint.as_os_str().as_encoded_bytes())?;
    let root = unsafe {
        libc::open(
            root_c.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if root < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut queue = VecDeque::from([(OwnedFd(root), mount.mountpoint.clone())]);
    let mut stats = ScanStats::default();
    let mut buffer = vec![0u8; 256 * 1024];
    while let Some((fd, parent_path)) = queue.pop_front() {
        let mut parent_stat: libc::stat = unsafe { std::mem::zeroed() };
        let parent_id = if unsafe { libc::fstat(fd.0, &mut parent_stat) } == 0 {
            parent_stat.st_ino as u64
        } else {
            0
        };
        loop {
            let mut attrs = AttrList {
                bitmapcount: ATTR_BIT_MAP_COUNT,
                reserved: 0,
                commonattr: ATTR_CMN_RETURNED_ATTRS | ATTR_CMN_NAME | ATTR_CMN_OBJTYPE,
                volattr: 0,
                dirattr: 0,
                fileattr: 0,
                forkattr: 0,
            };
            let count = unsafe {
                getattrlistbulk(
                    fd.0,
                    &mut attrs,
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                    0,
                )
            };
            if count < 0 {
                return Err(std::io::Error::last_os_error()).context("getattrlistbulk");
            }
            if count == 0 {
                break;
            }
            let mut pos = 0usize;
            for _ in 0..count {
                if pos + 36 > buffer.len() {
                    break;
                }
                let rec_len = u32::from_ne_bytes(buffer[pos..pos + 4].try_into().unwrap()) as usize;
                if rec_len < 36 || pos + rec_len > buffer.len() {
                    break;
                }
                // Record: length, returned attribute_set_t (20 bytes), then
                // requested common attrs in bit order: name attrreference,
                // object type. attr_dataoffset is relative to its own field.
                let attrref = pos + 24;
                let dataoff = i32::from_ne_bytes(buffer[attrref..attrref + 4].try_into().unwrap());
                let namelen =
                    u32::from_ne_bytes(buffer[attrref + 4..attrref + 8].try_into().unwrap())
                        as usize;
                let objtype =
                    u32::from_ne_bytes(buffer[attrref + 8..attrref + 12].try_into().unwrap());
                let name_start = (attrref as isize + dataoff as isize) as usize;
                if namelen == 0 || name_start >= pos + rec_len {
                    pos += rec_len;
                    continue;
                }
                let name_end = (name_start + namelen).min(pos + rec_len);
                let mut name = buffer[name_start..name_end].to_vec();
                if name.last() == Some(&0) {
                    name.pop();
                }
                if name == b"." || name == b".." || name.is_empty() {
                    pos += rec_len;
                    continue;
                }
                let cname = match CString::new(name.clone()) {
                    Ok(v) => v,
                    Err(_) => {
                        pos += rec_len;
                        continue;
                    }
                };
                let mut st: libc::stat = unsafe { std::mem::zeroed() };
                if unsafe {
                    libc::fstatat(fd.0, cname.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW)
                } < 0
                {
                    pos += rec_len;
                    continue;
                }
                let os = OsString::from_vec(name);
                let path = parent_path.join(os);
                let mode = st.st_mode as u32;
                let kind = match mode & libc::S_IFMT as u32 {
                    value if value == libc::S_IFDIR as u32 => FileKind::Dir,
                    value if value == libc::S_IFREG as u32 => FileKind::File,
                    value if value == libc::S_IFLNK as u32 => FileKind::Symlink,
                    _ => FileKind::Other,
                };
                if objtype == VDIR || kind == FileKind::Dir {
                    let child = unsafe {
                        libc::openat(
                            fd.0,
                            cname.as_ptr(),
                            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                        )
                    };
                    if child >= 0 {
                        queue.push_back((OwnedFd(child), path.clone()));
                    }
                    stats.dirs += 1;
                } else {
                    stats.files += 1;
                }
                sink(FileRecord {
                    path: path.to_string_lossy().into_owned().into_boxed_str(),
                    size: st.st_size.max(0) as u64,
                    disk: (st.st_blocks.max(0) as u64).saturating_mul(512),
                    mtime: st.st_mtime,
                    mode,
                    kind,
                    fs: mount.fs.clone(),
                    native_id: st.st_ino as u64,
                    native_parent: parent_id,
                    source: 0,
                });
                stats.records += 1;
                pos += rec_len;
            }
        }
    }
    stats.wall_ms = started.elapsed().as_millis() as u64;
    stats.detail =
        "fallback: getattrlistbulk bulk traversal (Spotlight disabled; never readdir)".into();
    Ok(stats)
}

