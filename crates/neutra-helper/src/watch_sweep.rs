//! Keeps the index current with Btrfs changes the closed-write watcher cannot
//! see: deletes, renames, new folders, other subvolumes. Every few seconds it
//! asks each subvolume what changed since the last sweep (a transaction-id
//! range search, so idle disks cost nothing) and applies the difference.

use crate::store::{append_suffix, DurableStore};
use crate::sweep_plan::{reconcile, Mounted};
use crate::MAX_PENDING_CHANGES;
use anyhow::{anyhow, Result};
use neutra_btrfs::Subvolume;
use neutra_core::{FsKind, MountInfo};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

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
        restore(&mut watched, &state);
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
    ) {
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
            save(&watched, &state);
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
                let mut moved = false;
                for item in watched.iter_mut() {
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
                    save(&watched, &state);
                }
                #[cfg(target_os = "linux")]
                unsafe {
                    libc::malloc_trim(0);
                }
            }
        });
    }
}

fn sweep_once(store: &Arc<RwLock<DurableStore>>, item: &mut Watched, source: u32) -> Result<bool> {
    let committed = item.sub.current_generation()?;
    if !item.active && committed <= item.since {
        return Ok(false);
    }
    let started = std::time::Instant::now();
    let changes = item.sub.changes_since(item.since)?;
    if changes.inodes.is_empty() && changes.dirs.is_empty() {
        let moved = committed > item.since;
        item.since = item.since.max(committed);
        item.active = false;
        return Ok(moved);
    }
    let mounted = Mounted {
        prefix: &item.prefix,
        relative_root: &item.relative_root,
        fs: &item.mount.fs,
        source,
    };
    let (path, generation) = {
        let guard = store
            .read()
            .map_err(|_| anyhow!("durable store lock poisoned"))?;
        (
            guard.path.clone(),
            guard
                .view()
                .ok_or_else(|| anyhow!("compact base is unavailable"))?
                .0
                .generation(),
        )
    };
    let browser = neutra_core::BrowserIndex::open(&path, generation).ok();
    let excluded = crate::scan::watch_exclusions(&path);
    let planned = if let Some(browser) = browser.as_ref() {
        // Catalog reads do not hold the event writer behind reconciliation.
        let base = neutra_core::CompactIndex::open_fast(&path)?;
        if base.generation() != generation {
            return Err(anyhow!("compact base changed during native reconciliation"));
        }
        reconcile(&item.sub, &changes, &base, None, &mounted, Some(browser))?
    } else {
        let guard = store
            .read()
            .map_err(|_| anyhow!("durable store lock poisoned"))?;
        let (base, delta) = guard
            .view()
            .ok_or_else(|| anyhow!("compact base is unavailable"))?;
        reconcile(&item.sub, &changes, base, Some(delta), &mounted, None)?
    };
    let planned = planned
        .into_iter()
        .filter(|change| {
            let path = match change {
                neutra_core::DeltaChange::Upsert(record) => &record.path,
                neutra_core::DeltaChange::Remove(path) => path,
            };
            !excluded
                .iter()
                .any(|excluded| Path::new(&**path).starts_with(excluded))
        })
        .collect::<Vec<_>>();
    for chunk in planned.chunks(MAX_PENDING_CHANGES) {
        let mut guard = store
            .write()
            .map_err(|_| anyhow!("durable store lock poisoned"))?;
        guard.apply_bounded(chunk.to_vec())?;
    }
    if !planned.is_empty() {
        tracing::debug!(changes = planned.len(), mount = %item.mount.mountpoint.display(), "sweep committed");
    }
    // A live leaf can be newer than the committed root. Keep the range
    // inclusive until that transaction commits, including writes after this
    // search, rather than skipping them with the newest observed leaf ID.
    let next = committed.max(item.since);
    let moved = next != item.since;
    item.since = next;
    item.active = changes.generation > committed;
    tracing::debug!(inodes = changes.inodes.len(), dirs = changes.dirs.len(), seconds = started.elapsed().as_secs_f64(), mount = %item.mount.mountpoint.display(), "sweep reconciled");
    Ok(moved)
}

fn open_subvolumes(mounts: &[MountInfo]) -> Result<Vec<Watched>> {
    let mut out = Vec::new();
    for mount in mounts {
        if mount.fs != FsKind::Btrfs {
            continue;
        }
        let sub = open_for(mount).ok_or_else(|| {
            anyhow!(
                "cannot open native Btrfs metadata at {}",
                mount.mountpoint.display()
            )
        })?;
        let since = sub.current_generation()?;
        let prefix = mount
            .mountpoint
            .to_string_lossy()
            .trim_end_matches('/')
            .to_string();
        let relative_root = sub.opened_dir_path()?;
        tracing::info!(mount = %mount.mountpoint.display(), tree = sub.tree_id(), "sweeping Btrfs subvolume");
        out.push(Watched {
            sub,
            mount: mount.clone(),
            prefix,
            relative_root,
            since,
            active: false,
        });
    }
    Ok(out)
}

/// Open a mount's subvolume, going through the user's home when the mount
/// root itself is not readable by the user and both sit on the same mount.
fn open_for(mount: &MountInfo) -> Option<Subvolume> {
    if let Ok(sub) = Subvolume::open(&mount.mountpoint) {
        return Some(sub);
    }
    use std::os::unix::fs::MetadataExt;
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let same =
        std::fs::metadata(&home).ok()?.dev() == std::fs::metadata(&mount.mountpoint).ok()?.dev();
    same.then(|| Subvolume::open(&home).ok()).flatten()
}

/// Keep each mount's last safely reconciled cursor across compaction/rebuild.
fn restore(watched: &mut [Watched], state: &Path) {
    let Ok(text) = std::fs::read_to_string(state) else {
        return;
    };
    for line in text.lines() {
        let mut parts = line.splitn(3, ' ');
        let (Some(tree), Some(since)) = (
            parts.next().and_then(|v| v.parse::<u64>().ok()),
            parts.next().and_then(|v| v.parse::<u64>().ok()),
        ) else {
            continue;
        };
        if let Some(mount) = parts.next() {
            if let Some(item) = watched.iter_mut().find(|item| {
                item.sub.tree_id() == tree && item.mount.mountpoint.to_string_lossy() == mount
            }) {
                item.since = item.since.min(since);
            }
        } else {
            for item in watched.iter_mut().filter(|item| item.sub.tree_id() == tree) {
                item.since = item.since.min(since);
            }
        }
    }
}

fn save(watched: &[Watched], state: &Path) {
    let mut text = String::new();
    for item in watched {
        text.push_str(&format!(
            "{} {} {}\n",
            item.sub.tree_id(),
            item.since,
            item.mount.mountpoint.to_string_lossy()
        ));
    }
    let temporary = append_suffix(state, ".tmp");
    if std::fs::write(&temporary, text).is_ok() {
        let _ = std::fs::rename(temporary, state);
    }
}
