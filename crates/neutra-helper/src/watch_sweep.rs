//! Keeps the index current with Btrfs changes the closed-write watcher cannot
//! see: deletes, renames, new folders, other subvolumes. Every few seconds it
//! asks each subvolume what changed since the last sweep (a transaction-id
//! range search, so idle disks cost nothing) and applies the difference.

use crate::store::{append_suffix, DurableStore};
use crate::sweep_plan::{reconcile, Mounted};
use anyhow::{anyhow, Result};
use neutra_btrfs::Subvolume;
use neutra_core::{FsKind, MountInfo};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

mod reconcile;
mod state;
use reconcile::sweep_once;
use state::{open_subvolumes, restore, save};

const ACTIVE_SWEEP_INTERVAL: Duration = Duration::from_secs(2);
const IDLE_SWEEP_INTERVAL: Duration = Duration::from_secs(5);

struct Watched {
    sub: Subvolume,
    mount: MountInfo,
    prefix: String,
    relative_root: String,
    since: u64,
    active: bool,
}

pub(crate) struct PreparedSweep {
    watched: Vec<Watched>,
    state: PathBuf,
    base_path: PathBuf,
}

impl PreparedSweep {
    pub(crate) fn prepare(mounts: &[MountInfo], base: &Path) -> Result<Self> {
        let mut watched = open_subvolumes(mounts)?;
        let state = append_suffix(base, ".sweep");
        restore(&mut watched, &state)?;
        Ok(Self {
            watched,
            state,
            base_path: base.to_path_buf(),
        })
    }

    pub(crate) fn start(
        self,
        store: Arc<RwLock<DurableStore>>,
        stale: Arc<AtomicBool>,
        source: u32,
        requested: Arc<AtomicBool>,
    ) -> Result<()> {
        save(&self.watched, &self.state)?;
        std::thread::spawn(move || {
            let Self {
                mut watched,
                state,
                base_path,
            } = self;
            if watched.is_empty() {
                tracing::info!("no Btrfs subvolume to sweep");
                return;
            }
            loop {
                let interval = if watched.iter().any(|item| item.active) {
                    ACTIVE_SWEEP_INTERVAL
                } else {
                    IDLE_SWEEP_INTERVAL
                };
                std::thread::sleep(interval);
                if stale.load(Ordering::Acquire) {
                    return;
                }
                let force = requested.swap(false, Ordering::AcqRel);
                let mut moved = false;
                for item in watched.iter_mut() {
                    item.active |= force;
                    match sweep_once(&store, item, source) {
                        Ok(changed) => moved |= changed,
                        Err(error) => {
                            let reason = format!(
                                "Native metadata reconciliation failed at {}: {error:#}",
                                item.mount.mountpoint.display()
                            );
                            tracing::error!("{reason}");
                            if let Err(error) =
                                crate::store::write_stale_marker(&base_path, &reason)
                            {
                                tracing::error!("cannot persist index failure: {error:#}");
                            }
                            stale.store(true, Ordering::Release);
                            return;
                        }
                    }
                }
                if moved {
                    if let Err(error) = save(&watched, &state) {
                        let reason = format!("cannot save native recovery cursor: {error:#}");
                        tracing::error!("{reason}");
                        if let Err(error) = crate::store::write_stale_marker(&base_path, &reason) {
                            tracing::error!("cannot persist index failure: {error:#}");
                        }
                        stale.store(true, Ordering::Release);
                        return;
                    }
                }
                #[cfg(target_os = "linux")]
                unsafe {
                    libc::malloc_trim(0);
                }
            }
        });
        Ok(())
    }
}
