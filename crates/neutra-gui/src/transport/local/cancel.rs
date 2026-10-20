use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

pub(super) struct Guard {
    finished: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Guard {
    pub(super) fn new(pid: u32, cancelled: Arc<AtomicBool>) -> Self {
        let finished = Arc::new(AtomicBool::new(false));
        #[cfg(target_os = "linux")]
        let thread = {
            let done = finished.clone();
            Some(std::thread::spawn(move || {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(7200);
                while !done.load(Ordering::Relaxed) {
                    if cancelled.load(Ordering::Relaxed) || std::time::Instant::now() >= deadline {
                        // The owner has not reaped this child, so its PID cannot be reused.
                        unsafe {
                            libc::kill(pid as libc::pid_t, libc::SIGKILL);
                        }
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            }))
        };
        #[cfg(not(target_os = "linux"))]
        let thread = {
            let _ = (pid, cancelled);
            None
        };
        Self { finished, thread }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.finished.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn cancellation_kills_the_owned_child_only() {
        let mut child = std::process::Command::new("/usr/bin/sleep")
            .arg("10")
            .spawn()
            .unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let guard = Guard::new(child.id(), cancelled.clone());
        cancelled.store(true, Ordering::Relaxed);
        assert!(!child.wait().unwrap().success());
        drop(guard);
    }
}
