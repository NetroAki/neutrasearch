use super::*;
use neutra_core::DeltaIndex;

pub(super) fn overlay(
    store: &Arc<RwLock<DurableStore>>,
    generation: u64,
) -> Result<Option<DeltaIndex>> {
    let path = {
        let guard = store
            .read()
            .map_err(|_| anyhow!("durable store lock poisoned"))?;
        if guard.delta.generation() != generation {
            return Ok(None);
        }
        guard.delta.path().to_path_buf()
    };
    let snapshot = DeltaIndex::open_search_snapshot(&path, generation)
        .or_else(|_| DeltaIndex::open_snapshot(&path, generation));
    match snapshot {
        Ok(delta) => Ok(Some(delta)),
        Err(error) => {
            let current = store
                .read()
                .map_err(|_| anyhow!("durable store lock poisoned"))?
                .delta
                .generation();
            if current != generation {
                return Ok(None);
            }
            Err(anyhow!(error).context("open native reconciliation delta snapshot"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{build_test_base, record, remove_store, store_paths};
    use neutra_core::DeltaChange;

    #[test]
    fn fallback_snapshot_does_not_hold_the_store_or_block_live_writes() {
        let (path, delta) = store_paths("replay-fallback-snapshot");
        build_test_base(&[record("/base.txt", 1)], &path).unwrap();
        let mut writer = DurableStore::open(&path).unwrap();
        writer.background_compaction = true;
        writer
            .apply_bounded(vec![DeltaChange::Upsert(record("/before.txt", 2))])
            .unwrap();
        let generation = writer.delta.generation();
        let store = Arc::new(RwLock::new(writer));
        let snapshot = overlay(&store, generation).unwrap().unwrap();
        let (done, received) = std::sync::mpsc::channel();
        let live = Arc::clone(&store);
        let worker = std::thread::spawn(move || {
            let result = live
                .write()
                .unwrap()
                .apply_bounded(vec![DeltaChange::Upsert(record("/after.txt", 3))]);
            done.send(result.is_ok()).unwrap();
        });
        assert!(received.recv_timeout(Duration::from_secs(5)).unwrap());
        worker.join().unwrap();
        assert!(snapshot.upsert_for("/before.txt").unwrap().is_some());
        assert!(snapshot.upsert_for("/after.txt").unwrap().is_none());
        assert!(store
            .read()
            .unwrap()
            .delta
            .upsert_for("/after.txt")
            .unwrap()
            .is_some());
        drop(snapshot);
        drop(store);
        remove_store(&path, &delta);
    }
}
