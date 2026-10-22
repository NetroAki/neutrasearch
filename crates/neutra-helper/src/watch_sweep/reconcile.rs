use super::*;

pub(super) fn sweep_once(
    store: &Arc<RwLock<DurableStore>>,
    item: &mut Watched,
    source: u32,
    gate: &WriteGate,
) -> Result<bool> {
    // ponytail: prefix conflicts replan the native range; persist subtree progress to bound retries.
    loop {
        if let Some(moved) = attempt(store, item, source, gate)? {
            return Ok(moved);
        }
    }
}

fn attempt(
    store: &Arc<RwLock<DurableStore>>,
    item: &mut Watched,
    source: u32,
    gate: &WriteGate,
) -> Result<Option<bool>> {
    let committed = item.sub.current_generation()?;
    if !item.active && committed <= item.since {
        return Ok(Some(false));
    }
    let (path, generation, cursor) = {
        let guard = store
            .read()
            .map_err(|_| anyhow!("durable store lock poisoned"))?;
        (
            guard.path.clone(),
            guard.delta.generation(),
            guard.delta.compaction_cursor(),
        )
    };
    let started = std::time::Instant::now();
    let changes = item.sub.changes_since(item.since)?;
    if changes.inodes.is_empty() && changes.dirs.is_empty() {
        let moved = committed > item.since;
        item.since = item.since.max(committed);
        item.active = false;
        return Ok(Some(moved));
    }
    let mounted = Mounted {
        prefix: &item.prefix,
        relative_root: &item.relative_root,
        fs: &item.mount.fs,
        source,
    };
    let browser = neutra_core::BrowserIndex::open(&path, generation).ok();
    let excluded = crate::scan::watch_exclusions(&path);
    let base = neutra_core::CompactIndex::open_fast(&path)?;
    if base.generation() != generation {
        return Ok(None);
    }
    let planned = if let Some(browser) = browser.as_ref() {
        reconcile(&item.sub, &changes, &base, None, &mounted, Some(browser))?
    } else {
        let Some(delta) = reader::overlay(store, generation)? else {
            return Ok(None);
        };
        reconcile(&item.sub, &changes, &base, Some(&delta), &mounted, None)?
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
    if !commit::apply(store, gate, pending::Pending::new(planned), cursor)? {
        return Ok(None);
    }
    // A live leaf can be newer than the committed root. Keep the range
    // inclusive until that transaction commits, including writes after this
    // search, rather than skipping them with the newest observed leaf ID.
    let next = committed.max(item.since);
    let moved = next != item.since;
    item.since = next;
    item.active = active;
    tracing::debug!(inodes = inode_count, dirs = directory_count, seconds = started.elapsed().as_secs_f64(), mount = %item.mount.mountpoint.display(), "sweep reconciled");
    Ok(Some(moved))
}
