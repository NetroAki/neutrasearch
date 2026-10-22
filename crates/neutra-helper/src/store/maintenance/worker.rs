use super::*;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, RwLock, Weak,
};
use std::time::{Duration, Instant};

impl DurableStore {
    pub(crate) fn start_maintenance(
        store: &Arc<RwLock<Self>>,
        stale: Arc<AtomicBool>,
    ) -> Result<()> {
        if !enable_background_compaction(store)? {
            return Ok(());
        }
        let weak = Arc::downgrade(store);
        std::thread::Builder::new()
            .name("index-compaction".into())
            .spawn(move || {
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| maintain(weak, stale)))
                    .is_err()
                {
                    tracing::error!("index maintenance panicked; restarting for durable recovery");
                    #[cfg(not(test))]
                    std::process::exit(1);
                }
            })
            .context("start index maintenance")?;
        Ok(())
    }
}

pub(super) fn enable_background_compaction(store: &Arc<RwLock<DurableStore>>) -> Result<bool> {
    let mut guard = store
        .write()
        .map_err(|_| anyhow::anyhow!("store lock poisoned"))?;
    if guard.background_compaction {
        return Ok(false);
    }
    guard.startup_compaction_deferred = false;
    guard.background_compaction = true;
    Ok(true)
}

fn maintain(weak: Weak<RwLock<DurableStore>>, stale: Arc<AtomicBool>) {
    let mut retry_at = Instant::now();
    loop {
        std::thread::sleep(Duration::from_secs(2));
        if stale.load(Ordering::Acquire) {
            return;
        }
        let Some(store) = weak.upgrade() else {
            return;
        };
        if Instant::now() < retry_at {
            continue;
        }
        if let Err(error) = maintain_once(&store, &stale) {
            tracing::error!(%error, "background index maintenance failed; retrying in ten minutes");
            let unavailable = store.read().map_or(true, |guard| guard.view().is_none());
            if unavailable {
                // The old or new durable pair is recoverable. Restart the
                // supervisor instead of marking valid metadata as stale.
                stale.store(true, Ordering::Release);
                #[cfg(not(test))]
                std::process::exit(1);
                #[cfg(test)]
                return;
            }
            retry_at = Instant::now() + Duration::from_secs(600);
        }
    }
}

fn maintain_once(store: &Arc<RwLock<DurableStore>>, stale: &AtomicBool) -> Result<()> {
    let frozen = store
        .read()
        .map_err(|_| anyhow::anyhow!("store lock poisoned"))?
        .freeze_compaction()?;
    let Some(frozen) = frozen else {
        return Ok(());
    };
    tracing::info!("background index compaction started");
    let prepared = frozen.build()?;
    if Arc::strong_count(store) == 1 {
        return Ok(());
    }
    commit_if_live(store, prepared, stale)
}

pub(super) fn commit_if_live(
    store: &Arc<RwLock<DurableStore>>,
    prepared: PreparedCompaction,
    stale: &AtomicBool,
) -> Result<()> {
    let mut guard = store
        .write()
        .map_err(|_| anyhow::anyhow!("store lock poisoned"))?;
    if stale.load(Ordering::Acquire) {
        return Ok(());
    }
    guard.publish_compaction(prepared)?;
    if stale.load(Ordering::Acquire) {
        write_stale_marker(
            &guard.path,
            "Native event coverage was lost during compaction",
        )?;
    }
    Ok(())
}
