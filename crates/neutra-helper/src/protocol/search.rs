use super::*;

pub(super) fn handle(
    query: neutra_core::Query,
    index: &Arc<RwLock<Index>>,
    durable: &Option<Arc<RwLock<DurableStore>>>,
    stale: &AtomicBool,
    out: &ProtocolOutput,
) -> Result<()> {
    if stale.load(Ordering::Acquire) {
        send(
            out,
            &HelperMsg::Error("index is stale; run a full native reindex before searching".into()),
        )?;
        return Ok(());
    }
    if let Err(error) = validate_query(&query) {
        send(out, &HelperMsg::Error(error.to_string()))?;
        return Ok(());
    }
    let (hits, stats) = if let Some(store) = &durable {
        let store = store.read().unwrap();
        if stale.load(Ordering::Acquire) {
            send(
                out,
                &HelperMsg::Error(
                    "index became stale during the query; rebuild and restart the service".into(),
                ),
            )?;
            return Ok(());
        }
        store.search(&query)?
    } else {
        index.read().unwrap().search(&query)?
    };
    send(
        out,
        &HelperMsg::SearchResult {
            hits: hits.into_iter().map(|hit| hit.record).collect(),
            wall_us: stats.wall_us,
        },
    )?;

    Ok(())
}
