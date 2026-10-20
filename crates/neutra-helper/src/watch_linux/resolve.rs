use super::*;

impl FanotifyWatcher {
    pub(super) fn resolve_named(
        &self,
        location: &mut Location,
    ) -> io::Result<(PathBuf, Option<libc::stat>, u64)> {
        let parent = open_handle(
            self.mount_fd.as_raw_fd(),
            &mut location.handle,
            libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )?;
        let parent_path = fd_path(parent.as_raw_fd())?;
        let mut parent_stat = zeroed_stat();
        // SAFETY: parent_stat points to writable storage for fstat.
        if unsafe { libc::fstat(parent.as_raw_fd(), &mut parent_stat) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let name = location.name.as_ref().expect("named location");
        if name.as_bytes() == b"." {
            let native_parent = parent_path
                .parent()
                .and_then(|path| std::fs::symlink_metadata(path).ok())
                .map(|metadata| {
                    use std::os::unix::fs::MetadataExt;
                    metadata.ino()
                })
                .unwrap_or(0);
            return Ok((parent_path, Some(parent_stat), native_parent));
        }
        let path = parent_path.join(name);
        let name = CString::new(name.as_bytes()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "fanotify name contains NUL")
        })?;
        let mut stat = zeroed_stat();
        // SAFETY: name is NUL-terminated, parent is a directory descriptor, and
        // stat points to writable storage.
        if unsafe {
            libc::fstatat(
                parent.as_raw_fd(),
                name.as_ptr(),
                &mut stat,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            let error = io::Error::last_os_error();
            if stale(&error) {
                return Ok((path, None, parent_stat.st_ino));
            }
            return Err(error);
        }
        Ok((path, Some(stat), parent_stat.st_ino))
    }

    pub(super) fn resolve_object(
        &self,
        location: &mut Location,
    ) -> io::Result<(PathBuf, libc::stat, u64)> {
        let object = open_handle(
            self.mount_fd.as_raw_fd(),
            &mut location.handle,
            libc::O_PATH | libc::O_CLOEXEC,
        )?;
        let mut path = fd_path(object.as_raw_fd())?;
        let mut stat = zeroed_stat();
        // SAFETY: stat points to writable storage for fstat.
        if unsafe { libc::fstat(object.as_raw_fd(), &mut stat) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if stat.st_nlink == 0 {
            if let Some(original) = path.as_os_str().as_bytes().strip_suffix(b" (deleted)") {
                path = PathBuf::from(OsString::from_vec(original.to_vec()));
            }
        }
        let parent = match &self.native_root {
            Some(root) => root.parent_inode(&path),
            None => crate::watch_mount::parent_inode(&path),
        };
        Ok((path, stat, parent))
    }
}

fn open_handle(mount_fd: i32, handle: &mut [u8], flags: i32) -> io::Result<File> {
    let slots = handle.len().div_ceil(std::mem::size_of::<u64>());
    let mut aligned = vec![0u64; slots];
    // SAFETY: this byte view covers the initialized u64 allocation.
    let bytes = unsafe {
        std::slice::from_raw_parts_mut(
            aligned.as_mut_ptr().cast::<u8>(),
            aligned.len() * std::mem::size_of::<u64>(),
        )
    };
    bytes[..handle.len()].copy_from_slice(handle);
    // SAFETY: parser validated the complete variable-sized file_handle, and the
    // aligned allocation remains alive through the syscall.
    let fd = unsafe {
        libc::open_by_handle_at(
            mount_fd,
            aligned.as_mut_ptr().cast::<libc::file_handle>(),
            flags,
        )
    };
    if fd < 0 {
        let source = io::Error::last_os_error();
        return Err(source);
    }
    // SAFETY: open_by_handle_at returned a new owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}
