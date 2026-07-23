use anyhow::{bail, Context, Result};
use neutra_core::{DeltaChange, FileKind, FileRecord, MountInfo};
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
}

impl FanotifyWatcher {
    pub fn open(mount: MountInfo, source: u32, excluded: Vec<PathBuf>) -> Result<Self> {
        if !mount.fs.is_indexable_local() {
            bail!("fanotify requires a local native filesystem lane");
        }
        let mount_fd = File::open(&mount.mountpoint)
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
        let path = CString::new(mount.mountpoint.as_os_str().as_bytes())
            .context("mount path contains NUL")?;
        let base_mask = libc::FAN_CREATE
            | libc::FAN_DELETE
            | libc::FAN_MOVED_FROM
            | libc::FAN_MOVED_TO
            | libc::FAN_ATTRIB
            | libc::FAN_CLOSE_WRITE
            | libc::FAN_ONDIR
            | libc::FAN_EVENT_ON_CHILD;
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
        if let Some(reason) = rescan_reason(&events, self.atomic_rename) {
            return Ok(WatchBatch::RescanRequired(reason));
        }
        let mut changes = BTreeMap::<String, DeltaChange>::new();
        for event in events {
            if self.collect_event(event, &mut changes)? {
                return Ok(WatchBatch::RescanRequired(
                    "an event path became unresolvable before delivery",
                ));
            }
        }
        Ok(WatchBatch::Changes(changes.into_values().collect()))
    }

    /// Returns true when a raced/deleted handle makes this batch ambiguous.
    fn collect_event(
        &self,
        event: ParsedEvent,
        changes: &mut BTreeMap<String, DeltaChange>,
    ) -> Result<bool> {
        let mut used_named_location = false;
        for mut location in event
            .locations
            .iter()
            .filter(|location| location.name.is_some())
            .cloned()
        {
            used_named_location = true;
            let resolved = self.resolve_named(&mut location);
            let (path, stat, parent) = match resolved {
                Ok(resolved) => resolved,
                Err(error) if stale(&error) => return Ok(true),
                Err(error) => return Err(error.into()),
            };
            if self.is_excluded(&path) {
                continue;
            }
            let remove = location.role == Role::Old
                || (location.role == Role::Current
                    && event.mask & (libc::FAN_DELETE | libc::FAN_MOVED_FROM) != 0);
            if remove {
                insert_remove(changes, &path);
            } else if location.role == Role::New
                || event.mask
                    & (libc::FAN_CREATE
                        | libc::FAN_MOVED_TO
                        | libc::FAN_ATTRIB
                        | libc::FAN_CLOSE_WRITE
                        | libc::FAN_RENAME)
                    != 0
            {
                if let Some(stat) = stat {
                    insert_upsert(
                        changes,
                        make_record(&path, &stat, parent, &self.mount, self.source),
                    );
                } else {
                    insert_remove(changes, &path);
                }
            }
        }
        if used_named_location {
            return Ok(false);
        }
        if event.mask & (libc::FAN_ATTRIB | libc::FAN_CLOSE_WRITE) == 0 {
            return Ok(false);
        }
        if let Some(mut location) = event
            .locations
            .into_iter()
            .find(|location| location.role == Role::Object)
        {
            match self.resolve_object(&mut location) {
                Ok((path, stat, parent)) if !self.is_excluded(&path) => insert_upsert(
                    changes,
                    make_record(&path, &stat, parent, &self.mount, self.source),
                ),
                Ok(_) => {}
                Err(error) if stale(&error) => return Ok(true),
                Err(error) => return Err(error.into()),
            }
        }
        Ok(false)
    }

    fn resolve_named(
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

    fn resolve_object(&self, location: &mut Location) -> io::Result<(PathBuf, libc::stat, u64)> {
        let object = open_handle(
            self.mount_fd.as_raw_fd(),
            &mut location.handle,
            libc::O_PATH | libc::O_CLOEXEC,
        )?;
        let path = fd_path(object.as_raw_fd())?;
        let mut stat = zeroed_stat();
        // SAFETY: stat points to writable storage for fstat.
        if unsafe { libc::fstat(object.as_raw_fd(), &mut stat) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if stat.st_nlink == 0 {
            return Err(io::Error::from_raw_os_error(libc::ENOENT));
        }
        let parent = path
            .parent()
            .and_then(|parent| std::fs::symlink_metadata(parent).ok())
            .map(|metadata| {
                use std::os::unix::fs::MetadataExt;
                metadata.ino()
            })
            .unwrap_or(0);
        Ok((path, stat, parent))
    }

    fn is_excluded(&self, path: &Path) -> bool {
        !path.starts_with(&self.mount.mountpoint)
            || self.excluded.iter().any(|excluded| path == excluded)
    }
}

fn fanotify_init(flags: u32, event_flags: i32) -> io::Result<i32> {
    // SAFETY: syscall has no borrowed output and returns a new descriptor.
    let fd = unsafe { libc::fanotify_init(flags, event_flags as u32) };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(fd)
    }
}

fn fanotify_mark(fd: i32, flags: u32, mask: u64, path: &CString) -> io::Result<()> {
    // SAFETY: path is a valid NUL-terminated string for the duration of the call.
    let result = unsafe { libc::fanotify_mark(fd, flags, mask, libc::AT_FDCWD, path.as_ptr()) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
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
        return Err(io::Error::new(
            source.kind(),
            format!(
                "open_by_handle_at requires CAP_DAC_READ_SEARCH and filesystem file-handle support: {source}"
            ),
        ));
    }
    // SAFETY: open_by_handle_at returned a new owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn fd_path(fd: i32) -> io::Result<PathBuf> {
    std::fs::read_link(format!("/proc/self/fd/{fd}"))
}

fn zeroed_stat() -> libc::stat {
    // SAFETY: all-zero is a valid initial byte pattern for an output stat buffer.
    unsafe { std::mem::zeroed() }
}

fn stale(error: &io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(libc::ENOENT) | Some(libc::ESTALE)
    )
}

fn make_record(
    path: &Path,
    stat: &libc::stat,
    parent: u64,
    mount: &MountInfo,
    source: u32,
) -> FileRecord {
    let kind = match stat.st_mode & libc::S_IFMT {
        libc::S_IFDIR => FileKind::Dir,
        libc::S_IFLNK => FileKind::Symlink,
        libc::S_IFREG => FileKind::File,
        _ => FileKind::Other,
    };
    FileRecord {
        path: path.to_string_lossy().into_owned().into_boxed_str(),
        size: stat.st_size.max(0) as u64,
        mtime: stat.st_mtime,
        mode: stat.st_mode,
        kind,
        fs: mount.fs.clone(),
        native_id: stat.st_ino,
        native_parent: parent,
        source,
    }
}

fn insert_remove(changes: &mut BTreeMap<String, DeltaChange>, path: &Path) {
    let path = path.to_string_lossy().into_owned();
    changes.insert(path.clone(), DeltaChange::Remove(path.into_boxed_str()));
}

fn insert_upsert(changes: &mut BTreeMap<String, DeltaChange>, record: FileRecord) {
    changes.insert(record.path.to_string(), DeltaChange::Upsert(record));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Object,
    Current,
    Old,
    New,
}

#[derive(Clone, Debug)]
struct Location {
    role: Role,
    handle: Vec<u8>,
    name: Option<OsString>,
}

#[derive(Debug)]
struct ParsedEvent {
    mask: u64,
    locations: Vec<Location>,
}

fn parse_events(bytes: &[u8]) -> io::Result<Vec<ParsedEvent>> {
    let mut events = Vec::new();
    let mut event_at = 0usize;
    while event_at < bytes.len() {
        if bytes.len() - event_at < META_LEN {
            return Err(invalid("truncated fanotify event metadata"));
        }
        let event_len = u32_at(bytes, event_at)? as usize;
        let metadata_len = u16_at(bytes, event_at + 6)? as usize;
        if bytes[event_at + 4] != libc::FANOTIFY_METADATA_VERSION {
            return Err(invalid("unsupported fanotify metadata version"));
        }
        if metadata_len < META_LEN || event_len < metadata_len {
            return Err(invalid("invalid fanotify event lengths"));
        }
        let event_end = event_at
            .checked_add(event_len)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| invalid("fanotify event exceeds read buffer"))?;
        let mask = u64_at(bytes, event_at + 8)?;
        let mut locations = Vec::new();
        let mut info_at = event_at + metadata_len;
        while info_at < event_end {
            if event_end - info_at < INFO_HEADER_LEN {
                return Err(invalid("truncated fanotify info header"));
            }
            let info_type = bytes[info_at];
            let info_len = u16_at(bytes, info_at + 2)? as usize;
            if info_len < INFO_HEADER_LEN || info_at + info_len > event_end {
                return Err(invalid("invalid fanotify info length"));
            }
            if let Some(role) = role_for(info_type) {
                locations.push(parse_location(
                    &bytes[info_at..info_at + info_len],
                    role,
                    info_type != libc::FAN_EVENT_INFO_TYPE_FID
                        && info_type != libc::FAN_EVENT_INFO_TYPE_DFID,
                )?);
            }
            info_at += info_len;
        }
        events.push(ParsedEvent { mask, locations });
        event_at = event_end;
    }
    Ok(events)
}

fn rescan_reason(events: &[ParsedEvent], atomic_rename: bool) -> Option<&'static str> {
    if events
        .iter()
        .any(|event| event.mask & libc::FAN_Q_OVERFLOW != 0)
    {
        return Some("fanotify queue overflowed");
    }
    if events.iter().any(|event| {
        event.mask & libc::FAN_ONDIR != 0 && event.mask & (libc::FAN_MOVE | libc::FAN_RENAME) != 0
    }) {
        return Some("directory rename requires descendant path reconciliation");
    }
    if !atomic_rename && events.iter().any(|event| event.mask & libc::FAN_MOVE != 0) {
        return Some("kernel reported an unpaired move event");
    }
    None
}

fn role_for(info_type: u8) -> Option<Role> {
    match info_type {
        libc::FAN_EVENT_INFO_TYPE_FID => Some(Role::Object),
        libc::FAN_EVENT_INFO_TYPE_DFID | libc::FAN_EVENT_INFO_TYPE_DFID_NAME => Some(Role::Current),
        libc::FAN_EVENT_INFO_TYPE_OLD_DFID_NAME => Some(Role::Old),
        libc::FAN_EVENT_INFO_TYPE_NEW_DFID_NAME => Some(Role::New),
        _ => None,
    }
}

fn parse_location(info: &[u8], role: Role, has_name: bool) -> io::Result<Location> {
    if info.len() < FID_PREFIX_LEN + FILE_HANDLE_HEADER_LEN {
        return Err(invalid("fanotify FID record is too short"));
    }
    let handle_bytes = u32_at(info, FID_PREFIX_LEN)? as usize;
    let handle_end = FID_PREFIX_LEN
        .checked_add(FILE_HANDLE_HEADER_LEN)
        .and_then(|start| start.checked_add(handle_bytes))
        .filter(|end| *end <= info.len())
        .ok_or_else(|| invalid("fanotify file handle exceeds info record"))?;
    let handle = info[FID_PREFIX_LEN..handle_end].to_vec();
    let name = if has_name {
        let rest = &info[handle_end..];
        let nul = rest
            .iter()
            .position(|byte| *byte == 0)
            .ok_or_else(|| invalid("fanotify name is not NUL-terminated"))?;
        Some(OsString::from_vec(rest[..nul].to_vec()))
    } else {
        None
    };
    Ok(Location { role, handle, name })
}

fn u16_at(bytes: &[u8], at: usize) -> io::Result<u16> {
    let value = bytes
        .get(at..at + 2)
        .ok_or_else(|| invalid("truncated u16"))?;
    Ok(u16::from_ne_bytes(value.try_into().unwrap()))
}

fn u32_at(bytes: &[u8], at: usize) -> io::Result<u32> {
    let value = bytes
        .get(at..at + 4)
        .ok_or_else(|| invalid("truncated u32"))?;
    Ok(u32::from_ne_bytes(value.try_into().unwrap()))
}

fn u64_at(bytes: &[u8], at: usize) -> io::Result<u64> {
    let value = bytes
        .get(at..at + 8)
        .ok_or_else(|| invalid("truncated u64"))?;
    Ok(u64::from_ne_bytes(value.try_into().unwrap()))
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_name_and_handle_with_bounds() {
        let handle_payload = [0xaa, 0xbb];
        let mut info = Vec::new();
        info.extend_from_slice(&[libc::FAN_EVENT_INFO_TYPE_DFID_NAME, 0, 0, 0]);
        info.extend_from_slice(&[0; 8]);
        info.extend_from_slice(&(handle_payload.len() as u32).to_ne_bytes());
        info.extend_from_slice(&7i32.to_ne_bytes());
        info.extend_from_slice(&handle_payload);
        info.extend_from_slice(b"report.txt\0");
        let info_len = info.len() as u16;
        info[2..4].copy_from_slice(&info_len.to_ne_bytes());

        let event_len = META_LEN + info.len();
        let mut event = vec![0; META_LEN];
        event[0..4].copy_from_slice(&(event_len as u32).to_ne_bytes());
        event[4] = libc::FANOTIFY_METADATA_VERSION;
        event[6..8].copy_from_slice(&(META_LEN as u16).to_ne_bytes());
        event[8..16].copy_from_slice(&libc::FAN_CREATE.to_ne_bytes());
        event.extend_from_slice(&info);

        let parsed = parse_events(&event).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].locations[0].role, Role::Current);
        assert_eq!(
            parsed[0].locations[0].name.as_deref(),
            Some(std::ffi::OsStr::new("report.txt"))
        );
        assert_eq!(parsed[0].locations[0].handle.len(), 10);
    }

    #[test]
    fn directory_renames_require_native_reindex() {
        let events = [ParsedEvent {
            mask: libc::FAN_RENAME | libc::FAN_ONDIR,
            locations: Vec::new(),
        }];
        assert_eq!(
            rescan_reason(&events, true),
            Some("directory rename requires descendant path reconciliation")
        );
    }

    #[test]
    fn legacy_unpaired_moves_require_native_reindex() {
        let events = [ParsedEvent {
            mask: libc::FAN_MOVED_FROM,
            locations: Vec::new(),
        }];
        assert_eq!(
            rescan_reason(&events, false),
            Some("kernel reported an unpaired move event")
        );
    }

    #[test]
    fn rejects_info_record_past_event_boundary() {
        let mut event = vec![0; META_LEN + INFO_HEADER_LEN];
        let len = event.len() as u32;
        event[0..4].copy_from_slice(&len.to_ne_bytes());
        event[4] = libc::FANOTIFY_METADATA_VERSION;
        event[6..8].copy_from_slice(&(META_LEN as u16).to_ne_bytes());
        event[META_LEN + 2..META_LEN + 4].copy_from_slice(&99u16.to_ne_bytes());
        assert!(parse_events(&event).is_err());
    }
}
