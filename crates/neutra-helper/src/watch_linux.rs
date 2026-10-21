use anyhow::{bail, Context, Result};
mod collection;
mod deletion_coverage;
mod events;
mod records;
mod resolve;
mod root;
use events::{parse_events, rescan_reason, validate_utf8_paths, Location, ParsedEvent, Role};
use neutra_core::{DeltaChange, FileKind, FileRecord, MountInfo};
pub(crate) use records::make_record;
use root::NativeRoot;
use std::collections::BTreeMap;
use std::ffi::{CString, OsString};
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

const META_LEN: usize = 24;
const INFO_HEADER_LEN: usize = 4;
const FID_PREFIX_LEN: usize = 12;
const FILE_HANDLE_HEADER_LEN: usize = 8;

pub enum WatchBatch {
    Changes(Vec<DeltaChange>),
    ReconcileRequired(&'static str),
    RescanRequired(&'static str),
}

pub struct FanotifyWatcher {
    events: File,
    mount_fd: File,
    mount: MountInfo,
    source: u32,
    excluded: Vec<PathBuf>,
    atomic_rename: bool,
    buffer: Vec<u8>,
    native_root: Option<NativeRoot>,
}

impl FanotifyWatcher {
    pub fn open(mount: MountInfo, source: u32, excluded: Vec<PathBuf>) -> Result<Self> {
        if mount.fs == neutra_core::FsKind::Btrfs {
            return Self::open_btrfs(mount, source, excluded);
        }
        Self::open_inner(mount, source, excluded, None)
    }

    pub(crate) fn open_btrfs(
        mount: MountInfo,
        source: u32,
        excluded: Vec<PathBuf>,
    ) -> Result<Self> {
        let root = NativeRoot::open(&mount)?;
        Self::open_inner(mount, source, excluded, Some(root))
    }

    fn open_inner(
        mount: MountInfo,
        source: u32,
        excluded: Vec<PathBuf>,
        native_root: Option<NativeRoot>,
    ) -> Result<Self> {
        if !mount.fs.is_indexable_local() {
            bail!("fanotify requires a local native filesystem lane");
        }
        let mark_path = native_root
            .as_ref()
            .map(|root| PathBuf::from(format!("/proc/self/fd/{}", root.file.as_raw_fd())))
            .unwrap_or_else(|| mount.mountpoint.clone());
        let mount_fd = File::open(&mark_path)
            .with_context(|| format!("open watched mount {}", mount.mountpoint.display()))?;
        let report_target =
            libc::FAN_CLOEXEC | libc::FAN_CLASS_NOTIF | libc::FAN_REPORT_DFID_NAME_TARGET;
        let report_names = libc::FAN_CLOEXEC
            | libc::FAN_CLASS_NOTIF
            | libc::FAN_REPORT_FID
            | libc::FAN_REPORT_DFID_NAME;
        let event_flags = libc::O_RDONLY | libc::O_CLOEXEC | libc::O_LARGEFILE;
        let fd = match fanotify_init(report_target, event_flags) {
            Ok(fd) => fd,
            Err(error) if error.raw_os_error() == Some(libc::EINVAL) => {
                fanotify_init(report_names, event_flags).context(
                    "fanotify_init requires CAP_SYS_ADMIN and a kernel with FID reporting",
                )?
            }
            Err(error) => {
                return Err(error).context(
                    "fanotify_init requires CAP_SYS_ADMIN and a kernel with FID reporting",
                )
            }
        };
        // SAFETY: fanotify_init returned a new owned descriptor.
        let events = unsafe { File::from_raw_fd(fd) };
        let path =
            CString::new(mark_path.as_os_str().as_bytes()).context("mount path contains NUL")?;
        let mut base_mask = libc::FAN_CREATE
            | libc::FAN_DELETE
            | libc::FAN_MOVED_FROM
            | libc::FAN_MOVED_TO
            | libc::FAN_ATTRIB
            | libc::FAN_CLOSE_WRITE
            | libc::FAN_ONDIR
            | libc::FAN_EVENT_ON_CHILD;
        if native_root.is_some() {
            // ponytail: directory moves need durable prefix deltas before enabling FAN_ONDIR here.
            base_mask &= !libc::FAN_ONDIR;
        }
        let mark_flags = libc::FAN_MARK_ADD | libc::FAN_MARK_FILESYSTEM;
        let atomic_rename = match fanotify_mark(
            events.as_raw_fd(),
            mark_flags,
            base_mask | libc::FAN_RENAME,
            &path,
        ) {
            Ok(()) => true,
            Err(error) if error.raw_os_error() == Some(libc::EINVAL) => {
                fanotify_mark(events.as_raw_fd(), mark_flags, base_mask, &path)?;
                false
            }
            Err(error) => {
                return Err(error).context(
                    "fanotify filesystem mark (requires CAP_SYS_ADMIN and file-handle support)",
                )
            }
        };
        Ok(Self {
            events,
            mount_fd,
            mount,
            source,
            excluded,
            atomic_rename,
            buffer: vec![0; 256 * 1024],
            native_root,
        })
    }

    pub fn read_batch(&mut self) -> Result<WatchBatch> {
        let bytes = loop {
            match self.events.read(&mut self.buffer) {
                Ok(bytes) => break bytes,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error.into()),
            }
        };
        if bytes == 0 {
            return Err(
                io::Error::new(io::ErrorKind::UnexpectedEof, "fanotify descriptor closed").into(),
            );
        }
        let events = parse_events(&self.buffer[..bytes])?;
        validate_utf8_paths(&events)?;
        if let Some(reason) = rescan_reason(&events, self.atomic_rename) {
            return Ok(if self.mount.fs == neutra_core::FsKind::Btrfs {
                WatchBatch::ReconcileRequired(reason)
            } else {
                WatchBatch::RescanRequired(reason)
            });
        }
        let covered_deletions = deletion_coverage::confirmed_named_deletions(self, &events)?;
        let mut changes = BTreeMap::<String, DeltaChange>::new();
        for event in events {
            if self.collect_event(event, &mut changes, &covered_deletions)? {
                let reason = "an event path became unresolvable before delivery";
                return Ok(if self.mount.fs == neutra_core::FsKind::Btrfs {
                    WatchBatch::ReconcileRequired(reason)
                } else {
                    WatchBatch::RescanRequired(reason)
                });
            }
        }
        Ok(WatchBatch::Changes(changes.into_values().collect()))
    }

    /// Block until events are readable or the timeout elapses. Returns true
    /// when a subsequent read_batch will not block. Used to coalesce event
    /// bursts before committing them.
    pub fn wait_readable(&self, timeout: std::time::Duration) -> io::Result<bool> {
        let mut fds = [libc::pollfd {
            fd: self.events.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        }];
        loop {
            let ready = unsafe { libc::poll(fds.as_mut_ptr(), 1, timeout.as_millis() as i32) };
            if ready < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            return Ok(ready > 0);
        }
    }

    /// Returns true when a raced/deleted handle makes this batch ambiguous.
    fn collect_event(
        &self,
        event: ParsedEvent,
        changes: &mut BTreeMap<String, DeltaChange>,
        covered_deletions: &std::collections::HashSet<Vec<u8>>,
    ) -> Result<bool> {
        if event
            .locations
            .iter()
            .any(|location| location.name.is_some())
        {
            self.collect_named(&event, changes)
        } else {
            self.collect_object(event, changes, covered_deletions)
        }
    }

    fn is_excluded(&self, path: &Path) -> bool {
        !path.starts_with(&self.mount.mountpoint)
            || crate::scan::is_watch_excluded(path, &self.excluded)
    }

    fn visible_path(&self, path: PathBuf) -> Option<PathBuf> {
        match &self.native_root {
            Some(root) => root.visible_path(&path),
            None => Some(path),
        }
    }
}

pub(crate) fn fanotify_init(flags: u32, event_flags: i32) -> io::Result<i32> {
    // SAFETY: syscall has no borrowed output and returns a new descriptor.
    let fd = unsafe { libc::fanotify_init(flags, event_flags as u32) };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(fd)
    }
}

pub(crate) fn fanotify_mark(fd: i32, flags: u32, mask: u64, path: &CString) -> io::Result<()> {
    // SAFETY: path is a valid NUL-terminated string for the duration of the call.
    let result = unsafe { libc::fanotify_mark(fd, flags, mask, libc::AT_FDCWD, path.as_ptr()) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub(crate) fn fd_path(fd: i32) -> io::Result<PathBuf> {
    std::fs::read_link(format!("/proc/self/fd/{fd}"))
}

pub(crate) fn zeroed_stat() -> libc::stat {
    // SAFETY: all-zero is a valid initial byte pattern for an output stat buffer.
    unsafe { std::mem::zeroed() }
}

fn stale(error: &io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(libc::ENOENT) | Some(libc::ESTALE)
    )
}

fn insert_remove(changes: &mut BTreeMap<String, DeltaChange>, path: &Path) {
    let Some(path) = path.to_str() else {
        return;
    };
    let path = path.to_owned();
    changes.insert(path.clone(), DeltaChange::Remove(path.into_boxed_str()));
}

pub(crate) fn insert_upsert(changes: &mut BTreeMap<String, DeltaChange>, record: FileRecord) {
    changes.insert(record.path.to_string(), DeltaChange::Upsert(record));
}
