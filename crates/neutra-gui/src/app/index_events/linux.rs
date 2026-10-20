use super::*;
use std::{
    ffi::CString,
    fs::File,
    io::{self, Read},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::ffi::OsStrExt,
    },
    time::Duration,
};

pub(super) fn start(path: &Path, sender: EventSender) -> io::Result<IndexEvents> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("index has no parent directory"))?;
    let parent = CString::new(parent.as_os_str().as_bytes())?;
    let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_fd(fd) };
    let mask = libc::IN_MODIFY
        | libc::IN_CLOSE_WRITE
        | libc::IN_MOVED_TO
        | libc::IN_DELETE
        | libc::IN_CREATE;
    if unsafe { libc::inotify_add_watch(fd, parent.as_ptr(), mask) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let base = path
        .file_name()
        .ok_or_else(|| io::Error::other("index has no file name"))?
        .as_bytes()
        .to_vec();
    let delta = path
        .with_extension("delta")
        .file_name()
        .unwrap()
        .as_bytes()
        .to_vec();
    let mut stale = base.clone();
    stale.extend_from_slice(b".stale");
    let mut status = base.clone();
    status.extend_from_slice(b".watch-status");
    let active = Arc::new(AtomicBool::new(true));
    let running = active.clone();
    std::thread::Builder::new()
        .name("index-notifications".into())
        .spawn(move || run(file, [base, delta, stale, status], running, sender))?;
    Ok(IndexEvents { active })
}

fn run(mut file: File, names: [Vec<u8>; 4], running: Arc<AtomicBool>, sender: EventSender) {
    let mut buffer = [0u8; 8192];
    while running.load(Ordering::Acquire) {
        let mut poll = libc::pollfd {
            fd: file.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut poll, 1, 1000) };
        if ready == 0 {
            continue;
        }
        if ready < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            continue;
        }
        let bytes = match file.read(&mut buffer) {
            Ok(bytes) if bytes > 0 => bytes,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
            _ => break,
        };
        let changed = relevant_change(&buffer[..bytes], &names, &running);
        if changed && sender.send(Event::IndexChanged).is_err() {
            break;
        }
        // Continuous writes coalesce to five UI wakeups per second.
        if changed {
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    running.store(false, Ordering::Release);
    let _ = sender.send(Event::IndexChanged);
}

fn relevant_change(buffer: &[u8], names: &[Vec<u8>; 4], running: &AtomicBool) -> bool {
    let mut changed = false;
    let mut offset = 0;
    while offset + size_of::<libc::inotify_event>() <= buffer.len() {
        let event = unsafe {
            std::ptr::read_unaligned(buffer.as_ptr().add(offset).cast::<libc::inotify_event>())
        };
        let start = offset + size_of::<libc::inotify_event>();
        let end = start + event.len as usize;
        if end > buffer.len() {
            break;
        }
        let name = buffer[start..end]
            .split(|byte| *byte == 0)
            .next()
            .unwrap_or_default();
        changed |= names.iter().any(|wanted| wanted.as_slice() == name)
            || event.mask & (libc::IN_Q_OVERFLOW | libc::IN_IGNORED) != 0;
        if event.mask & libc::IN_IGNORED != 0 {
            running.store(false, Ordering::Release);
        }
        offset = end;
    }
    changed
}
