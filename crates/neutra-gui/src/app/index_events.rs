#[cfg_attr(not(target_os = "linux"), allow(dead_code, unused_imports))]
use super::{Event, EventSender};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

pub(crate) struct IndexEvents {
    active: Arc<AtomicBool>,
}

#[cfg(all(test, target_os = "linux"))]
mod tests;

impl Drop for IndexEvents {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
    }
}

impl IndexEvents {
    pub(crate) fn active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    pub(crate) fn start(path: &Path, sender: EventSender) -> std::io::Result<Self> {
        #[cfg(target_os = "linux")]
        {
            linux::start(path, sender)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (path, sender);
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "native index notification is unavailable",
            ))
        }
    }
}

#[cfg(target_os = "linux")]
mod linux;
