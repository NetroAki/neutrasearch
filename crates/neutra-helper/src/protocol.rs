//! The framed protocol loop: Hello handshake, optional client
//! authentication, command dispatch, and the watch-thread lifecycle.
//! Split from `main` so entry points stay separate from wire handling.

pub(crate) use crate::scan::ProtocolOutput;
use crate::scan::{
    launch_scans, prepare_scan, reap_scan_threads, validate_delta_changes, validate_query,
};
use crate::store::{write_stale_marker, DurableStore};
#[cfg(target_os = "linux")]
use crate::{watch_linux, watch_mount, MAX_PENDING_CHANGES, WATCH_DEBOUNCE};
use anyhow::{Context, Result};
use neutra_core::proto::{
    read_frame, write_frame, ClientMsg, HelperMsg, HELPER_BUILD, PROTO_VERSION,
};
use neutra_core::{DeltaChange, Index};
use std::collections::BTreeMap;
use std::io::{BufWriter, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

mod framing;
pub(crate) use framing::run_protocol;
#[cfg(any(test, target_os = "windows"))]
pub use framing::run_protocol_with_auth;
mod dispatch;
mod state;

mod watch_commit;
#[cfg(target_os = "linux")]
pub(crate) use watch_commit::start_native_watch;

fn send(out: &ProtocolOutput, msg: &HelperMsg) -> Result<()> {
    let mut w = out.lock().unwrap();
    write_frame(&mut *w, msg)?;
    Ok(())
}

pub(crate) fn send_lossy(out: &ProtocolOutput, msg: &HelperMsg) {
    if let Err(e) = send(out, msg) {
        tracing::warn!("failed to send frame: {e}");
    }
}

#[cfg(all(test, target_os = "linux"))]
mod watch_tests;

mod delta;
mod directory;
mod handshake;
mod search;
