//! Keeps the index current with Btrfs changes the closed-write watcher cannot
//! see: deletes, renames, new folders, other subvolumes. Every few seconds it
//! asks each subvolume what changed since the last sweep (a transaction-id
//! range search, so idle disks cost nothing) and applies the difference.

use crate::store::{append_suffix, DurableStore};
use crate::sweep_plan::{reconcile, Churn, Mounted};
use crate::MAX_PENDING_CHANGES;
use anyhow::{anyhow, Result};
use neutra_btrfs::Subvolume;
use neutra_core::{FsKind, MountInfo};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

const SWEEP_INTERVAL: Duration = Duration::from_secs(20);

struct Watched {
    sub: Subvolume,
    mount: MountInfo,
    prefix: String,
    since: u64,
    churn: Churn,
}

pub(crate) fn start(store: Arc<RwLock<DurableStore>>, stale: Arc<AtomicBool>, source: u32) {
    std::thread::spawn(move || {
        let Some((base_path, generation)) = store
            .read()
            .ok()
            .and_then(|guard| guard.view().map(|(base, _)| (guard.path.clone(), base.generation())))
        else {
            return;
        };
        let mut watched = open_subvolumes();
        if watched.is_empty() {
            tracing::info!("no Btrfs subvolume to sweep");
            return;
        }
        let state = append_suffix(&base_path, ".sweep");
        restore(&mut watched, &state, generation);
        save(&watched, &state, generation);
        loop {
            std::thread::sleep(SWEEP_INTERVAL);
            if stale.load(Ordering::Acquire) {
                return;
            }
            let mut moved = false;
            for item in watched.iter_mut() {
                match sweep_once(&store, item, source) {
                    Ok(changed) => moved |= changed,
                    Err(error) => tracing::warn!(mount = %item.mount.mountpoint.display(), "sweep failed: {error:#}"),
                }
            }
            if moved {
                save(&watched, &state, generation);
            }
        }
    });
}

fn sweep_once(store: &Arc<RwLock<DurableStore>>, item: &mut Watched, source: u32) -> Result<bool> {
    item.churn.begin_sweep();
    let changes = item.sub.changes_since(item.since)?;
    if changes.inodes.is_empty() && changes.dirs.is_empty() {
        return Ok(false);
    }
    let mounted = Mounted { prefix: &item.prefix, fs: &item.mount.fs, source };
    let planned = {
        let guard = store.read().map_err(|_| anyhow!("durable store lock poisoned"))?;
        let (base, delta) = guard.view().ok_or_else(|| anyhow!("compact base is unavailable"))?;
        reconcile(&item.sub, &changes, base, Some(delta), &mounted, &mut item.churn)?
    };
    for chunk in planned.chunks(MAX_PENDING_CHANGES) {
        let mut guard = store.write().map_err(|_| anyhow!("durable store lock poisoned"))?;
        guard.apply_bounded(chunk.to_vec())?;
    }
    if !planned.is_empty() {
        tracing::debug!(changes = planned.len(), mount = %item.mount.mountpoint.display(), "sweep committed");
    }
    let moved = changes.generation != item.since;
    item.since = changes.generation;
    Ok(moved)
}

fn open_subvolumes() -> Vec<Watched> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for mount in neutra_core::mounts::system_mounts().unwrap_or_default() {
        if mount.fs != FsKind::Btrfs {
            continue;
        }
        let Some(sub) = open_for(&mount) else { continue };
        if !seen.insert(sub.tree_id()) {
            continue;
        }
        let Ok(since) = sub.current_generation() else { continue };
        let prefix = mount.mountpoint.to_string_lossy().trim_end_matches('/').to_string();
        tracing::info!(mount = %mount.mountpoint.display(), tree = sub.tree_id(), "sweeping Btrfs subvolume");
        out.push(Watched { sub, mount, prefix, since, churn: Churn::default() });
    }
    out
}

/// Open a mount's subvolume, going through the user's home when the mount
/// root itself is not readable by the user and both sit on the same mount.
fn open_for(mount: &MountInfo) -> Option<Subvolume> {
    if let Ok(sub) = Subvolume::open(&mount.mountpoint) {
        return Some(sub);
    }
    use std::os::unix::fs::MetadataExt;
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let same = std::fs::metadata(&home).ok()?.dev() == std::fs::metadata(&mount.mountpoint).ok()?.dev();
    same.then(|| Subvolume::open(&home).ok()).flatten()
}

/// State is "index generation" then one "tree generation" line per subvolume.
/// It is trusted only for the base it was written against.
fn restore(watched: &mut [Watched], state: &Path, generation: u64) {
    let Ok(text) = std::fs::read_to_string(state) else { return };
    let mut lines = text.lines();
    if lines.next().and_then(|line| line.parse::<u64>().ok()) != Some(generation) {
        return;
    }
    for line in lines {
        let mut parts = line.split_whitespace();
        let (Some(tree), Some(since)) = (parts.next().and_then(|v| v.parse::<u64>().ok()), parts.next().and_then(|v| v.parse::<u64>().ok())) else { continue };
        if let Some(item) = watched.iter_mut().find(|item| item.sub.tree_id() == tree) {
            item.since = item.since.min(since);
        }
    }
}

fn save(watched: &[Watched], state: &Path, generation: u64) {
    let mut text = format!("{generation}\n");
    for item in watched {
        text.push_str(&format!("{} {}\n", item.sub.tree_id(), item.since));
    }
    let _ = std::fs::write(state, text);
}
