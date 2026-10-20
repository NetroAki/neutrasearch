use super::*;
#[cfg(target_os = "linux")]
mod writes;
#[cfg(target_os = "linux")]
use writes::commit_changes;

#[cfg(target_os = "linux")]
pub(crate) fn start_native_watch(
    mut watcher: Box<dyn watch_mount::Watch>,
    store: Arc<RwLock<DurableStore>>,
    stale: Arc<AtomicBool>,
    requested: Arc<AtomicBool>,
) {
    let base_path = match store.read() {
        Ok(store) => store.path.clone(),
        Err(_) => {
            stale.store(true, Ordering::Release);
            tracing::error!("cannot start native watch: durable store lock poisoned");
            return;
        }
    };
    std::thread::spawn(move || {
        let mut pending = BTreeMap::<String, DeltaChange>::new();
        loop {
            match watcher.read_batch() {
                Ok(watch_linux::WatchBatch::Changes(changes)) => {
                    merge_watched_changes(&mut pending, changes);
                }
                Ok(watch_linux::WatchBatch::ReconcileRequired(reason)) => {
                    request_reconciliation(&requested, reason);
                }
                Ok(watch_linux::WatchBatch::RescanRequired(reason)) => {
                    fail_closed(&stale, &base_path, anyhow::anyhow!(reason));
                    break;
                }
                Err(error) => {
                    fail_closed(&stale, &base_path, error);
                    break;
                }
            }
            if pending.is_empty() {
                continue;
            }
            if let Err(error) = drain_watch_burst(watcher.as_mut(), &mut pending, &requested) {
                fail_closed(&stale, &base_path, error);
                break;
            }
            let changes = std::mem::take(&mut pending)
                .into_values()
                .collect::<Vec<_>>();
            if let Err(error) = commit_changes(&store, &stale, &changes) {
                fail_closed(&stale, &base_path, error);
                break;
            }
        }
    });
}

#[cfg(target_os = "linux")]
pub(super) fn drain_watch_burst(
    watcher: &mut dyn watch_mount::Watch,
    pending: &mut BTreeMap<String, DeltaChange>,
    requested: &AtomicBool,
) -> Result<()> {
    // Continuous writes must not postpone publication indefinitely.
    let deadline = std::time::Instant::now() + WATCH_DEBOUNCE;
    while pending.len() < MAX_PENDING_CHANGES {
        let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) else {
            break;
        };
        if !watcher.wait_readable(remaining)? {
            break;
        }
        match watcher.read_batch()? {
            watch_linux::WatchBatch::Changes(changes) => merge_watched_changes(pending, changes),
            watch_linux::WatchBatch::ReconcileRequired(reason) => {
                request_reconciliation(requested, reason)
            }
            watch_linux::WatchBatch::RescanRequired(reason) => anyhow::bail!(reason),
        }
    }
    Ok(())
}

fn request_reconciliation(requested: &AtomicBool, reason: &str) {
    requested.store(true, Ordering::Release);
    tracing::warn!("native event gap: {reason}; replaying Btrfs metadata from the saved cursor");
}

fn merge_watched_changes(pending: &mut BTreeMap<String, DeltaChange>, changes: Vec<DeltaChange>) {
    for change in changes {
        if let DeltaChange::RemovePrefix(path) = &change {
            pending.retain(|_, earlier| {
                !neutra_core::paths::path_in_portable_root(change_path(earlier), path)
            });
        }
        let path = change_path(&change);
        let key = if matches!(change, DeltaChange::RemovePrefix(_)) {
            format!("\0{path}")
        } else {
            path.to_owned()
        };
        pending.insert(key, change);
    }
}

fn change_path(change: &DeltaChange) -> &str {
    match change {
        DeltaChange::Upsert(record) => &record.path,
        DeltaChange::Remove(path) | DeltaChange::RemovePrefix(path) => path,
    }
}

fn fail_closed(stale: &AtomicBool, base_path: &std::path::Path, error: anyhow::Error) {
    stale.store(true, Ordering::Release);
    if let Err(marker_error) = write_stale_marker(base_path, &format!("{error:#}")) {
        tracing::error!("failed to persist stale marker: {marker_error:#}");
    }
    tracing::error!("native watch stopped: {error:#}");
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn recreation_keeps_the_prefix_and_only_later_descendant_changes() {
        let mut pending = BTreeMap::new();
        merge_watched_changes(
            &mut pending,
            vec![
                DeltaChange::Remove("/d/old".into()),
                DeltaChange::RemovePrefix("/d".into()),
                DeltaChange::Remove("/d/new".into()),
                DeltaChange::Remove("/d".into()),
            ],
        );
        let changes: Vec<_> = pending.into_values().collect();
        assert_eq!(changes.len(), 3);
        assert!(matches!(&changes[0], DeltaChange::RemovePrefix(path) if &**path == "/d"));
        assert!(!changes.iter().any(|change| change_path(change) == "/d/old"));
    }
}
