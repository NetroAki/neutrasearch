//! neutrasearch-helper: the privileged (or platform-native) scanning daemon.
//!
//! Speaks neutra-core::proto over stdin/stdout (framed bincode). The same
//! binary is auto-provisioned onto Linux/Windows/macOS file servers, so all
//! logging goes to stderr — stdout is protocol-only.
//!
//! Lanes per platform (no filesystem walking anywhere):
//!   linux   btrfs (TREE_SEARCH ioctl) · ext4 (libext2fs raw device) ·
//!           ntfs (raw $MFT parse) · zfs (snapshot+ZAP, experimental)
//!   windows ntfs ($MFT via volume handle)
//!   macos   Spotlight namespace and indexed-path attributes

mod cli;
mod process_setup;
mod protocol;
mod scan;
mod store;
#[cfg(target_os = "linux")]
mod sweep_plan;
#[cfg(target_os = "linux")]
mod watch_linux;
#[cfg(target_os = "linux")]
mod watch_mount;
#[cfg(target_os = "linux")]
mod watch_process;
#[cfg(target_os = "linux")]
mod watch_session;
#[cfg(target_os = "linux")]
mod watch_sweep;
use protocol::run_protocol;
#[cfg(test)]
use scan::path_has_component_prefix;
#[cfg(test)]
use store::write_compaction_marker;
#[cfg(test)]
use store::DurableStore;
#[cfg(target_os = "windows")]
mod windows_service;
use anyhow::{Context, Result};
#[cfg(test)]
use neutra_core::CompactIndex;
#[cfg(test)]
use std::io::{Cursor, Write};
use std::time::Duration;

/// Watched-event bursts (builds, checkouts) commit once after this quiet window.
#[cfg(target_os = "linux")]
pub(crate) const WATCH_DEBOUNCE: Duration = Duration::from_millis(250);
#[cfg(target_os = "linux")]
pub(crate) const MAX_PENDING_CHANGES: usize = crate::scan::MAX_DELTA_CHANGES;

fn main() -> Result<()> {
    process_setup::initialize()?;
    #[cfg(target_os = "linux")]
    if std::env::args().nth(1).as_deref() == Some("--watch-events") {
        return watch_process::run();
    }
    #[cfg(target_os = "windows")]
    if std::env::args().nth(1).as_deref() == Some("--windows-service") {
        return windows_service::run();
    }

    let Some(cli::Options {
        serve_index,
        watch_mount,
    }) = cli::parse()?
    else {
        return Ok(());
    };

    let serve_index = serve_index
        .map(|path| {
            std::fs::canonicalize(&path)
                .with_context(|| format!("resolve compact index {}", path.display()))
        })
        .transpose()?;
    let watch_mount = watch_mount
        .map(|(path, source)| {
            std::fs::canonicalize(&path)
                .with_context(|| format!("resolve watched mount {}", path.display()))
                .map(|path| (path, source))
        })
        .transpose()?;

    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "neutrasearch_helper=info,neutra_helper=info".into()),
        )
        .init();

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut rin = stdin.lock();
    run_protocol(&mut rin, Box::new(stdout), serve_index, watch_mount, None)
}

#[cfg(test)]
mod tests;
