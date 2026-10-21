use std::os::fd::{FromRawFd, OwnedFd};
use std::process::Command;

pub(super) fn after_parent_exit() {
    let parent = std::env::args()
        .nth(2)
        .and_then(|value| value.parse::<i32>().ok());
    let Some(parent) = parent.filter(|pid| *pid > 1) else {
        return;
    };
    if !wait_for_exit(parent) {
        return;
    }
    let unit = format!("neutrasearch-watch@{}.service", unsafe { libc::getuid() });
    let _ = Command::new("/usr/bin/timeout")
        .args([
            "30",
            "/usr/bin/systemctl",
            "--no-ask-password",
            "--no-block",
            "start",
            &unit,
        ])
        .status();
}

fn wait_for_exit(pid: i32) -> bool {
    let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) } as i32;
    if raw < 0 {
        return std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
    }
    let _owned = unsafe { OwnedFd::from_raw_fd(raw) };
    let mut descriptor = libc::pollfd {
        fd: raw,
        events: libc::POLLIN,
        revents: 0,
    };
    unsafe { libc::poll(&mut descriptor, 1, -1) > 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_for_the_exact_process_to_exit() {
        let mut child = Command::new("/usr/bin/sleep").arg("0.02").spawn().unwrap();
        assert!(wait_for_exit(child.id() as i32));
        child.wait().unwrap();
    }
}
