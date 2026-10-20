use std::fs::File;
use std::io;
use std::os::fd::FromRawFd;

pub(super) const META_LEN: usize = 24;
pub(super) const MAX_READ_EVENTS: usize = 64;
// FAN_REPORT_FD_ERROR is newer than the libc constants used by this crate.
pub(super) const FD_ERROR_FLAG: u32 = 0x0000_2000;

pub(super) struct Event {
    pub(super) mask: u64,
    pub(super) pid: i32,
    pub(super) file: Option<File>,
    pub(super) error: Option<i32>,
}

pub(super) fn is_vanished(error: &io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(libc::ENOENT) | Some(libc::ESTALE)
    )
}

pub(super) fn parse_events(bytes: &[u8], fd_error_reporting: bool) -> io::Result<Vec<Event>> {
    let mut events = Vec::new();
    let mut at = 0usize;
    while at < bytes.len() {
        if bytes.len() - at < META_LEN {
            return Err(invalid("truncated fanotify event metadata"));
        }
        let mask = u64_at(bytes, at + 8)?;
        let raw_fd = i32_at(bytes, at + 16)?;
        // Acquire ownership as soon as the complete fixed header is present,
        // before validating lengths or version so malformed frames still close.
        let (file, error) = if raw_fd >= 0 {
            // SAFETY: successful fanotify metadata transfers this descriptor to us.
            (Some(unsafe { File::from_raw_fd(raw_fd) }), None)
        } else if mask & libc::FAN_Q_OVERFLOW != 0
            || (!fd_error_reporting && raw_fd == libc::FAN_NOFD)
        {
            (None, None)
        } else {
            (
                None,
                Some(raw_fd.checked_neg().ok_or_else(|| {
                    invalid("fanotify event descriptor error cannot be represented")
                })?),
            )
        };
        let len = u32_at(bytes, at)? as usize;
        let metadata_len = u16_at(bytes, at + 6)? as usize;
        if bytes[at + 4] != libc::FANOTIFY_METADATA_VERSION
            || metadata_len < META_LEN
            || len < metadata_len
        {
            return Err(invalid("invalid fanotify event metadata"));
        }
        let end = at
            .checked_add(len)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| invalid("fanotify event exceeds read buffer"))?;
        let pid = i32_at(bytes, at + 20)?;
        events.push(Event {
            mask,
            pid,
            file,
            error,
        });
        at = end;
    }
    Ok(events)
}

fn u16_at(bytes: &[u8], at: usize) -> io::Result<u16> {
    Ok(u16::from_ne_bytes(bytes[at..at + 2].try_into().unwrap()))
}
fn u32_at(bytes: &[u8], at: usize) -> io::Result<u32> {
    Ok(u32::from_ne_bytes(bytes[at..at + 4].try_into().unwrap()))
}
fn u64_at(bytes: &[u8], at: usize) -> io::Result<u64> {
    Ok(u64::from_ne_bytes(bytes[at..at + 8].try_into().unwrap()))
}
fn i32_at(bytes: &[u8], at: usize) -> io::Result<i32> {
    Ok(i32::from_ne_bytes(bytes[at..at + 4].try_into().unwrap()))
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::{AsRawFd, IntoRawFd};

    fn frame(mask: u64, fd: i32) -> [u8; META_LEN] {
        let mut bytes = [0; META_LEN];
        bytes[0..4].copy_from_slice(&(META_LEN as u32).to_ne_bytes());
        bytes[4] = libc::FANOTIFY_METADATA_VERSION;
        bytes[6..8].copy_from_slice(&(META_LEN as u16).to_ne_bytes());
        bytes[8..16].copy_from_slice(&mask.to_ne_bytes());
        bytes[16..20].copy_from_slice(&fd.to_ne_bytes());
        bytes
    }

    #[test]
    fn vanished_error_descriptor_is_per_event_and_later_event_survives() {
        let mut bytes = frame(libc::FAN_CLOSE_WRITE, -libc::ENOENT).to_vec();
        let later = File::open("/dev/null").unwrap();
        let later_fd = later.into_raw_fd();
        bytes.extend_from_slice(&frame(libc::FAN_CLOSE_WRITE, later_fd));
        let events = parse_events(&bytes, true).unwrap();
        assert_eq!(events[0].error, Some(libc::ENOENT));
        assert_eq!(
            events[1].file.as_ref().map(AsRawFd::as_raw_fd),
            Some(later_fd)
        );
    }

    #[test]
    fn other_negative_event_descriptor_is_reported() {
        let events = parse_events(&frame(libc::FAN_CLOSE_WRITE, -libc::EBADF), true).unwrap();
        assert_eq!(events[0].error, Some(libc::EBADF));
    }

    #[test]
    fn fd_error_mode_reports_permission_error_instead_of_legacy_nofd() {
        let events = parse_events(&frame(libc::FAN_CLOSE_WRITE, libc::FAN_NOFD), true).unwrap();
        assert_eq!(events[0].error, Some(libc::EPERM));
    }

    #[test]
    fn legacy_mode_accepts_nofd_sentinel() {
        let events = parse_events(&frame(libc::FAN_CLOSE_WRITE, libc::FAN_NOFD), false).unwrap();
        assert_eq!(events[0].error, None);
        assert!(events[0].file.is_none());
    }

    #[test]
    fn rejects_unrepresentable_error_descriptor() {
        assert!(parse_events(&frame(libc::FAN_CLOSE_WRITE, i32::MIN), true).is_err());
    }

    #[test]
    fn malformed_later_frame_closes_descriptors_from_earlier_frames() {
        let file = File::open("/dev/null").unwrap();
        let fd = file.into_raw_fd();
        let mut bytes = frame(libc::FAN_CLOSE_WRITE, fd).to_vec();
        let mut malformed = frame(0, libc::FAN_NOFD);
        malformed[0..4].copy_from_slice(&0u32.to_ne_bytes());
        bytes.extend_from_slice(&malformed);
        assert!(parse_events(&bytes, true).is_err());
        assert!(unsafe { libc::fcntl(fd, libc::F_GETFD) } < 0);
    }

    #[test]
    fn malformed_frame_closes_its_own_descriptor() {
        let file = File::open("/dev/null").unwrap();
        let fd = file.into_raw_fd();
        let mut bytes = frame(libc::FAN_CLOSE_WRITE, fd);
        bytes[0..4].copy_from_slice(&1u32.to_ne_bytes());
        assert!(parse_events(&bytes, true).is_err());
        assert!(unsafe { libc::fcntl(fd, libc::F_GETFD) } < 0);
    }
}
