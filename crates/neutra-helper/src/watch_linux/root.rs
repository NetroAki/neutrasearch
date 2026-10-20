use super::*;

pub(super) struct NativeRoot {
    pub(super) file: File,
    relative: PathBuf,
    visible: PathBuf,
    tree_id: u64,
}

impl NativeRoot {
    pub(super) fn open(mount: &MountInfo) -> Result<Self> {
        let raw = std::fs::read_to_string("/proc/self/mountinfo")?;
        let relative = neutra_core::mounts::mount_root(&raw, &mount.mountpoint)
            .context("cannot identify watched Btrfs subvolume root")?;
        // The anonymous mount is never attached to the user's namespace.
        let context = owned_fd(unsafe { libc::syscall(libc::SYS_fsopen, c"btrfs".as_ptr(), 1) })?;
        configure(
            context.as_raw_fd(),
            c"source",
            &CString::new(mount.device.as_bytes())?,
        )?;
        configure(context.as_raw_fd(), c"subvolid", c"5")?;
        let created = unsafe {
            libc::syscall(
                libc::SYS_fsconfig,
                context.as_raw_fd(),
                6,
                std::ptr::null::<libc::c_char>(),
                std::ptr::null::<libc::c_char>(),
                0,
            )
        };
        if created < 0 {
            return Err(io::Error::last_os_error()).context("create Btrfs metadata mount");
        }
        let attrs = libc::MOUNT_ATTR_RDONLY
            | libc::MOUNT_ATTR_NOSUID
            | libc::MOUNT_ATTR_NODEV
            | libc::MOUNT_ATTR_NOEXEC;
        let detached =
            owned_fd(unsafe { libc::syscall(libc::SYS_fsmount, context.as_raw_fd(), 1, attrs) })?;
        let file = File::open(format!("/proc/self/fd/{}", detached.as_raw_fd()))?;
        Ok(Self {
            file,
            relative: PathBuf::from(relative),
            visible: mount.mountpoint.clone(),
            tree_id: neutra_btrfs::Subvolume::open(&mount.mountpoint)?.tree_id(),
        })
    }

    pub(super) fn visible_path(&self, path: &Path) -> Option<PathBuf> {
        path.strip_prefix(&self.relative)
            .ok()
            .map(|tail| self.visible.join(tail))
    }

    pub(super) fn accepts_handle(&self, handle: &[u8]) -> io::Result<bool> {
        // Linux exportfs Btrfs handles share the object/root/generation prefix.
        if handle.len() < 28
            || !matches!(
                i32::from_ne_bytes(handle[4..8].try_into().unwrap()),
                0x4d..=0x4f
            )
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported Btrfs event handle",
            ));
        }
        Ok(u64::from_ne_bytes(handle[16..24].try_into().unwrap()) == self.tree_id)
    }

    pub(super) fn parent_inode(&self, path: &Path) -> u64 {
        use std::os::unix::fs::MetadataExt;
        let Some(parent) = path
            .parent()
            .and_then(|parent| parent.strip_prefix("/").ok())
        else {
            return 0;
        };
        std::fs::metadata(
            PathBuf::from(format!("/proc/self/fd/{}", self.file.as_raw_fd())).join(parent),
        )
        .map_or(0, |metadata| metadata.ino())
    }
}

fn configure(fd: i32, key: &std::ffi::CStr, value: &std::ffi::CStr) -> Result<()> {
    // FSCONFIG_SET_STRING copies both NUL-terminated strings before returning.
    let rc = unsafe { libc::syscall(libc::SYS_fsconfig, fd, 1, key.as_ptr(), value.as_ptr(), 0) };
    if rc < 0 {
        return Err(io::Error::last_os_error()).context("configure Btrfs metadata mount");
    }
    Ok(())
}

fn owned_fd(fd: libc::c_long) -> io::Result<File> {
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // Each successful mount syscall returns a new owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd as i32) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_only_the_watched_subvolume_or_bind_root() {
        let root = NativeRoot {
            file: File::open("/dev/null").unwrap(),
            relative: "/@home/user/data".into(),
            visible: "/data".into(),
            tree_id: 257,
        };
        assert_eq!(
            root.visible_path(Path::new("/@home/user/data/music/song.wav")),
            Some("/data/music/song.wav".into())
        );
        assert_eq!(
            root.visible_path(Path::new("/@home/user/data")),
            Some("/data/".into())
        );
        assert!(root
            .visible_path(Path::new("/@home/user/database/song.wav"))
            .is_none());
        assert!(root.visible_path(Path::new("/@/etc/passwd")).is_none());
        let mut handle = vec![0; 28];
        handle[0..4].copy_from_slice(&20u32.to_ne_bytes());
        handle[4..8].copy_from_slice(&0x4di32.to_ne_bytes());
        handle[16..24].copy_from_slice(&257u64.to_ne_bytes());
        assert!(root.accepts_handle(&handle).unwrap());
        handle[16..24].copy_from_slice(&256u64.to_ne_bytes());
        assert!(!root.accepts_handle(&handle).unwrap());
        assert!(root.accepts_handle(&[]).is_err());
    }
}
