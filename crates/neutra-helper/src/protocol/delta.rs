use super::*;

pub(super) fn handle(
    changes: Vec<DeltaChange>,
    durable: &Option<Arc<RwLock<DurableStore>>>,
    stale: &AtomicBool,
    out: &ProtocolOutput,
) -> Result<()> {
    if let Err(error) = validate_delta_changes(&changes) {
        send(out, &HelperMsg::Error(error.to_string()))?;
        return Ok(());
    }
    if stale.load(Ordering::Acquire) {
        send(
            out,
            &HelperMsg::Error(
                "index is stale; run a full native reindex before applying changes".into(),
            ),
        )?;
        return Ok(());
    }
    let Some(store) = &durable else {
        send(
            out,
            &HelperMsg::Error("ApplyDelta requires 'neutrasearch serve --index INDEX.nsx'".into()),
        )?;
        return Ok(());
    };
    let mut store = store.write().unwrap();
    if stale.load(Ordering::Acquire) {
        send(
            out,
            &HelperMsg::Error(
                "index became stale before the update; rebuild and restart the service".into(),
            ),
        )?;
        return Ok(());
    }
    match store.apply_bounded(changes) {
        Ok(applied) => {
            if let Some(compacted) = applied.compacted {
                tracing::info!(
                    records = compacted.records,
                    bytes = compacted.bytes,
                    "compacted delta into replacement base"
                );
            }
            send(
                out,
                &HelperMsg::DeltaApplied {
                    changes: applied.changes,
                    wal_bytes: applied.wal_bytes,
                    needs_compaction: false,
                },
            )?;
        }
        Err(error) => {
            stale.store(true, Ordering::Release);
            if let Err(marker_error) = write_stale_marker(&store.path, &error.to_string()) {
                tracing::error!("failed to persist stale marker: {marker_error:#}");
            }
            send(
                out,
                &HelperMsg::Error(format!(
                    "delta commit or compaction failed; index disabled until rebuild: {error:#}"
                )),
            )?;
        }
    }

    Ok(())
}
