//! Watcher for mounts that refuse filesystem-wide fanotify marks, such as
//! Btrfs subvolume mounts (the kernel returns EXDEV). A mount mark cannot
//! report creates, deletes or renames, but it does report every closed write,
//! which covers saving a file. Btrfs structural changes use generation sweeps;
//! FUSE NTFS structural changes require a journal reader.

use crate::watch_linux::{
    fanotify_init, fanotify_mark, fd_path, insert_upsert, make_record, zeroed_stat,
    FanotifyWatcher, WatchBatch,
};
use anyhow::{Context, Result};
use neutra_core::{DeltaChange, MountInfo};
use std::collections::BTreeMap;
use std::ffi::CString;
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

mod events;
use events::{is_vanished, parse_events, FD_ERROR_FLAG};

/// What the native watch loop needs from either kind of watcher.
pub(crate) trait Watch: Send + 'static {
    fn read_batch(&mut self) -> Result<WatchBatch>;
    fn wait_readable(&self, timeout: Duration) -> io::Result<bool>;
}

impl Watch for FanotifyWatcher {
    fn read_batch(&mut self) -> Result<WatchBatch> {
        FanotifyWatcher::read_batch(self)
    }
    fn wait_readable(&self, timeout: Duration) -> io::Result<bool> {
        FanotifyWatcher::wait_readable(self, timeout)
    }
}

pub(crate) struct MountWatcher {
    events: File,
    fd_error_reporting: bool,
    mount: MountInfo,
    source: u32,
    excluded: Vec<PathBuf>,
    buffer: Vec<u8>,
}

/// True when the filesystem-wide watcher cannot be used on this mount: the
/// kernel refuses subvolume marks (EXDEV), or an unprivileged user cannot
/// open the mount directory to resolve file handles (EACCES).
pub(crate) fn needs_mount_mark(error: &anyhow::Error, fs: &neutra_core::FsKind) -> bool {
    error
        .chain()
        .filter_map(|cause| cause.downcast_ref::<io::Error>())
        .any(|cause| {
            matches!(cause.raw_os_error(), Some(libc::EXDEV) | Some(libc::EACCES))
                || (matches!(fs, neutra_core::FsKind::Btrfs | neutra_core::FsKind::Ntfs)
                    && cause.raw_os_error() == Some(libc::ENODEV))
        })
}

impl MountWatcher {
    pub(crate) fn open(mount: MountInfo, source: u32, excluded: Vec<PathBuf>) -> Result<Self> {
        let (fd, fd_error_reporting) = match fanotify_init(
            libc::FAN_CLOEXEC | libc::FAN_CLASS_NOTIF | FD_ERROR_FLAG,
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_LARGEFILE,
        ) {
            Ok(fd) => (fd, true),
            Err(error) if error.raw_os_error() == Some(libc::EINVAL) => (
                fanotify_init(
                    libc::FAN_CLOEXEC | libc::FAN_CLASS_NOTIF,
                    libc::O_RDONLY | libc::O_CLOEXEC | libc::O_LARGEFILE,
                )?,
                false,
            ),
            Err(error) => return Err(error).context("fanotify_init requires CAP_SYS_ADMIN"),
        };
        // SAFETY: fanotify_init returned a new owned descriptor.
        let events = unsafe { File::from_raw_fd(fd) };
        let mark = |target: &Path| -> io::Result<()> {
            let path = CString::new(target.as_os_str().as_bytes())
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))?;
            fanotify_mark(
                events.as_raw_fd(),
                libc::FAN_MARK_ADD | libc::FAN_MARK_MOUNT,
                libc::FAN_CLOSE_WRITE,
                &path,
            )
        };
        // A mount mark needs read access to the path it names but covers the
        // whole mount, so an unreadable mount root can be marked through the
        // user's home when it lives on the same mount.
        match mark(&mount.mountpoint) {
            Err(error) if error.raw_os_error() == Some(libc::EACCES) => {
                let home = same_mount_home(&mount.mountpoint).ok_or(error)?;
                mark(&home)
            }
            other => other,
        }
        .context("fanotify mount mark")?;
        Ok(Self {
            events,
            fd_error_reporting,
            mount,
            source,
            excluded,
            buffer: vec![0; events::META_LEN * events::MAX_READ_EVENTS],
        })
    }

    fn record(&self, file: &File, changes: &mut BTreeMap<String, DeltaChange>) -> Result<()> {
        let Ok(path) = fd_path(file.as_raw_fd()) else {
            return Ok(());
        };
        let Some(path_text) = path.to_str() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "fanotify event path is not valid UTF-8",
            )
            .into());
        };
        let gone = path_text.ends_with(" (deleted)");
        if gone
            || !path.starts_with(&self.mount.mountpoint)
            || crate::scan::is_watch_excluded(&path, &self.excluded)
        {
            return Ok(());
        }
        let mut stat = zeroed_stat();
        // SAFETY: stat points to writable storage for fstat.
        if unsafe { libc::fstat(file.as_raw_fd(), &mut stat) } != 0 {
            return Ok(());
        }
        let parent = parent_inode(&path);
        let record =
            make_record(&path, &stat, parent, &self.mount, self.source).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "fanotify event path is not valid UTF-8",
                )
            })?;
        insert_upsert(changes, record);
        Ok(())
    }
}

fn same_mount_home(mountpoint: &Path) -> Option<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let same = std::fs::metadata(&home).ok()?.dev() == std::fs::metadata(mountpoint).ok()?.dev();
    same.then_some(home)
}

pub(crate) fn parent_inode(path: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    path.parent()
        .and_then(|parent| std::fs::symlink_metadata(parent).ok())
        .map_or(0, |metadata| metadata.ino())
}

impl Watch for MountWatcher {
    fn read_batch(&mut self) -> Result<WatchBatch> {
        let bytes = loop {
            match self.events.read(&mut self.buffer) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "fanotify descriptor closed",
                    )
                    .into());
                }
                Ok(bytes) => break bytes,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if is_vanished(&error) => {
                    return Ok(WatchBatch::Changes(Vec::new()));
                }
                Err(error) => return Err(error.into()),
            }
        };
        let own = std::process::id() as i32;
        let mut changes = BTreeMap::new();
        let parsed = parse_events(&self.buffer[..bytes], self.fd_error_reporting)?;
        for event in parsed {
            if event.mask & libc::FAN_Q_OVERFLOW != 0 {
                return Ok(WatchBatch::RescanRequired("the event queue overflowed"));
            }
            if let Some(file) = event.file {
                if event.pid != own {
                    self.record(&file, &mut changes)?;
                }
            } else if let Some(error) = event.error {
                if !is_vanished(&io::Error::from_raw_os_error(error)) {
                    return Err(io::Error::from_raw_os_error(error).into());
                }
            }
        }
        Ok(WatchBatch::Changes(changes.into_values().collect()))
    }

    fn wait_readable(&self, timeout: Duration) -> io::Result<bool> {
        let mut fds = [libc::pollfd {
            fd: self.events.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        }];
        loop {
            // SAFETY: fds points to one initialised pollfd.
            let ready = unsafe { libc::poll(fds.as_mut_ptr(), 1, timeout.as_millis() as i32) };
            if ready >= 0 {
                return Ok(ready > 0);
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
}
