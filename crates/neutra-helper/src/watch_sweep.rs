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
mod seed;
mod state;
use reconcile::sweep_once;
use seed::prepare_seed;
use state::{open_subvolumes, save};

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
    base_generation: Option<u64>,
    seed_identity: u32,
}

impl PreparedSweep {
    pub(crate) fn prepare(mounts: &[MountInfo], base: &Path) -> Result<Self> {
        let mut watched = open_subvolumes(mounts)?;
        let state = append_suffix(base, ".sweep");
        let binding = prepare_seed(&mut watched, base, &state)?;
        Ok(Self {
            watched,
            state,
            base_path: base.to_path_buf(),
            base_generation: binding.generation,
            seed_identity: binding.identity,
        })
    }

    pub(crate) fn start(
        self,
        store: Arc<RwLock<DurableStore>>,
        stale: Arc<AtomicBool>,
        source: u32,
        requested: Arc<AtomicBool>,
    ) -> Result<()> {
        let mut prepared = self;
        // Watch readers are attached by WatchSession before this method. Replay
        // the pre-ingest cursor synchronously so WatchReady cannot race it.
        if let Some(expected) = prepared.base_generation {
            let actual = neutra_core::CompactIndex::generation_on_disk(&prepared.base_path)?;
            anyhow::ensure!(
                actual == expected,
                "compact base changed after native cursor preparation; retry watch startup"
            );
        }
        for item in prepared.watched.iter_mut().filter(|item| item.active) {
            if let Err(error) = sweep_once(&store, item, source) {
                let reason = format!(
                    "Native metadata replay failed at {}: {error:#}",
                    item.mount.mountpoint.display()
                );
                let _ = crate::store::write_stale_marker(&prepared.base_path, &reason);
                stale.store(true, Ordering::Release);
                return Err(anyhow!("{reason}"));
            }
        }
        if let Some(expected) = prepared.base_generation {
            let actual = neutra_core::CompactIndex::generation_on_disk(&prepared.base_path)?;
            anyhow::ensure!(
                actual == expected,
                "compact base changed during native replay; retry watch startup"
            );
        }
        save(&prepared.watched, &prepared.state, prepared.seed_identity)?;
        std::thread::spawn(move || {
            let Self {
                mut watched,
                state,
                base_path,
                seed_identity,
                ..
            } = prepared;
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
                    if let Err(error) = save(&watched, &state, seed_identity) {
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
