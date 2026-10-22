use super::*;

pub(super) fn commit_changes(
    store: &Arc<RwLock<DurableStore>>,
    stale: &AtomicBool,
    changes: &[DeltaChange],
    gate: &crate::watch_sweep::WriteGate,
) -> Result<()> {
    for chunk in changes.chunks(DurableStore::LIVE_COMMIT_BATCH) {
        let _ticket = gate.ticket()?;
        let mut guard = store
            .write()
            .map_err(|_| anyhow::anyhow!("durable store lock poisoned"))?;
        anyhow::ensure!(
            !stale.load(Ordering::Acquire),
            "native event coverage is unavailable"
        );
        let applied = guard
            .apply_bounded(chunk.to_vec())
            .inspect_err(|_| stale.store(true, Ordering::Release))?;
        tracing::debug!(
            changes = applied.changes,
            wal_bytes = applied.wal_bytes,
            "native watch batch committed"
        );
        drop(guard);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{build_test_base, record, remove_store, store_paths};

    #[test]
    fn native_bursts_keep_prefix_removals_and_later_saves_across_commits() {
        let (path, delta) = store_paths("native-commit-burst");
        build_test_base(&[record("/folder/old.txt", 1)], &path).unwrap();
        let store = Arc::new(RwLock::new(DurableStore::open(&path).unwrap()));
        store.write().unwrap().background_compaction = true;
        let mut changes = vec![DeltaChange::RemovePrefix("/folder".into())];
        for n in 0..600 {
            changes.push(DeltaChange::Upsert(record(
                &format!("/folder/{n:03}.txt"),
                n,
            )));
        }
        changes.push(DeltaChange::Remove("/folder/003.txt".into()));
        changes.push(DeltaChange::Upsert(record("/folder/004.txt", 999)));
        let gate = crate::watch_sweep::WriteGate::default();
        commit_changes(&store, &AtomicBool::new(false), &changes, &gate).unwrap();
        let guard = store.read().unwrap();
        assert!(guard.delta.is_removed("/folder/old.txt").unwrap());
        assert!(guard.delta.is_removed("/folder/003.txt").unwrap());
        assert_eq!(
            guard
                .delta
                .upsert_for("/folder/004.txt")
                .unwrap()
                .unwrap()
                .size,
            999
        );
        drop(guard);
        drop(store);
        let reopened = DurableStore::open(&path).unwrap();
        assert_eq!(reopened.delta.change_count().unwrap(), 601);
        drop(reopened);
        remove_store(&path, &delta);
    }
}
