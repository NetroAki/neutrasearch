use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) fn accept(listener: UnixListener) {
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => receive(&mut stream),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => return,
        }
    }
}

fn receive(stream: &mut UnixStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
    let mut message = [0; 6];
    if stream.read_exact(&mut message).is_err() || message != MESSAGE {
        return;
    }
    FOCUS_PENDING.store(true, Ordering::Release);
    if let Some(ctx) = CONTEXT.get() {
        ctx.request_repaint();
    }
}

static FOCUS_PENDING: AtomicBool = AtomicBool::new(false);
static CONTEXT: std::sync::OnceLock<egui::Context> = std::sync::OnceLock::new();

pub(in crate::quick_search) fn set_context(ctx: egui::Context) {
    let _ = CONTEXT.set(ctx);
}

pub(in crate::quick_search) fn take_focus_request() -> bool {
    FOCUS_PENDING.swap(false, Ordering::AcqRel)
}
