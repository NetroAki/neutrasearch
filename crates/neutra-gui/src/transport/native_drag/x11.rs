//! XDND v5 copy-only source.  The worker owns a private X connection so it
//! never consumes events from winit's connection.

use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::{BeginError, DragAction, DragEvent, NativeDragSource};
mod atoms;
mod events;
mod selection;
mod session;
mod target;
use session::Session;
use x11rb::connection::Connection;

const DRAG_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_OFFER: usize = super::MAX_OFFER_BYTES;

#[derive(Clone, Copy)]
enum Command {
    Cancel,
}

pub(super) fn begin(window: u32, bytes: Vec<u8>) -> Result<Box<dyn NativeDragSource>, BeginError> {
    if bytes.len() > MAX_OFFER {
        return Err(BeginError::Failed("X11 drag offer exceeds 1 MiB".into()));
    }
    if bytes.is_empty() {
        return Err(BeginError::Failed("X11 drag offer is empty".into()));
    }
    let (event_tx, event_rx) = mpsc::channel();
    let (command_tx, command_rx) = mpsc::channel();
    let worker = thread::Builder::new()
        .name("neutrasearch-xdnd-source".into())
        .spawn(move || run(window, bytes, command_rx, event_tx))
        .map_err(|e| BeginError::Failed(format!("cannot start X11 drag worker: {e}")))?;
    Ok(Box::new(Source {
        events: event_rx,
        command: command_tx,
        worker: Some(worker),
        terminal: false,
    }))
}

struct Source {
    events: Receiver<DragEvent>,
    command: Sender<Command>,
    worker: Option<JoinHandle<()>>,
    terminal: bool,
}

impl NativeDragSource for Source {
    fn poll(&mut self) -> Vec<DragEvent> {
        let mut out = Vec::new();
        loop {
            match self.events.try_recv() {
                Ok(event) => {
                    if matches!(
                        event,
                        DragEvent::Finished { .. } | DragEvent::Cancelled | DragEvent::Failed(_)
                    ) {
                        self.terminal = true;
                    }
                    out.push(event);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if !self.terminal {
                        out.push(DragEvent::Failed(
                            "X11 drag worker stopped unexpectedly".into(),
                        ));
                        self.terminal = true;
                    }
                    break;
                }
            }
        }
        if self.terminal {
            self.join();
        }
        out
    }
    fn cancel(&mut self) {
        let _ = self.command.send(Command::Cancel);
    }
}

impl Source {
    fn join(&mut self) {
        if self
            .worker
            .as_ref()
            .is_some_and(|worker| worker.is_finished())
        {
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }
}
impl Drop for Source {
    fn drop(&mut self) {
        let _ = self.command.send(Command::Cancel);
        self.join();
    }
}

fn run(window: u32, bytes: Vec<u8>, commands: Receiver<Command>, events: Sender<DragEvent>) {
    let result = (|| {
        let mut session = Session::new(window, bytes)?;
        events.send(DragEvent::Started).map_err(err)?;
        loop {
            if session.started.elapsed() >= DRAG_TIMEOUT {
                return Ok(DragEvent::Failed("File drag timed out".into()));
            }
            match commands.try_recv() {
                Ok(Command::Cancel) | Err(TryRecvError::Disconnected) => {
                    return Ok(DragEvent::Cancelled)
                }
                Err(TryRecvError::Empty) => {}
            }
            while let Some(event) = session.conn.poll_for_event().map_err(err)? {
                if let Some(terminal) = session.handle(event, &events)? {
                    return Ok(terminal);
                }
            }
            if session.release_timed_out() {
                return Ok(DragEvent::Cancelled);
            }
            thread::sleep(Duration::from_millis(8));
        }
    })();
    let _ = events.send(result.unwrap_or_else(DragEvent::Failed));
}

fn err(error: impl std::fmt::Display) -> String {
    error.to_string()
}
