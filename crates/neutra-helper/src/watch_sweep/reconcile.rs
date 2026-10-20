use super::*;

pub(super) fn sweep_once(
    store: &Arc<RwLock<DurableStore>>,
    item: &mut Watched,
    source: u32,
) -> Result<bool> {
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
    let active = changes.generation > committed;
    let inode_count = changes.inodes.len();
    let directory_count = changes.dirs.len();
    // Release metadata buffers before catalog writes duplicate each update chunk.
    drop(changes);
    let planned = planned
        .into_iter()
        .filter(|change| {
            let path = match change {
                neutra_core::DeltaChange::Upsert(record) => &record.path,
                neutra_core::DeltaChange::Remove(path)
                | neutra_core::DeltaChange::RemovePrefix(path) => path,
            };
            !crate::scan::is_watch_excluded(Path::new(&**path), &excluded)
        })
        .collect::<Vec<_>>();
    for chunk in planned.chunks(DurableStore::LIVE_COMMIT_BATCH) {
        {
            let mut guard = store
                .write()
                .map_err(|_| anyhow!("durable store lock poisoned"))?;
            guard.apply_bounded(chunk.to_vec())?;
        }
        std::thread::yield_now();
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
    item.active = active;
    tracing::debug!(inodes = inode_count, dirs = directory_count, seconds = started.elapsed().as_secs_f64(), mount = %item.mount.mountpoint.display(), "sweep reconciled");
    Ok(moved)
}
