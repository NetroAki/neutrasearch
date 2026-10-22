use super::{pending::Pending, *};
use neutra_core::CompactionCursor;

/// False requires replanning without advancing the native metadata cursor.
pub(super) fn apply(
    store: &Arc<RwLock<DurableStore>>,
    gate: &WriteGate,
    mut pending: Pending,
    mut cursor: CompactionCursor,
) -> Result<bool> {
    while !pending.is_empty() {
        let _ticket = gate.ticket()?;
        let mut guard = store
            .write()
            .map_err(|_| anyhow!("durable store lock poisoned"))?;
        match guard.delta.visit_changes_after(cursor, |change| {
            pending.supersede(change);
            Ok(())
        }) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(false),
            Err(error) => return Err(error.into()),
        }
        let batch = pending.batch(DurableStore::LIVE_COMMIT_BATCH);
        if !batch.is_empty() {
            guard.apply_bounded(batch)?;
        }
        // The same writer turn covers validation, publication and this cursor,
        // so only later live events are considered by the next batch.
        cursor = guard.delta.compaction_cursor();
    }
    Ok(!pending.retry)
}

#[cfg(test)]
mod tests;
