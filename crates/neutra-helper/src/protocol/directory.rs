use super::*;

pub(super) fn handle(
    source: u32,
    path: String,
    durable: &Option<Arc<RwLock<DurableStore>>>,
    stale: &AtomicBool,
    out: &ProtocolOutput,
) -> Result<()> {
    let Some(store) = &durable else {
        send(
            out,
            &HelperMsg::Error(
                "DirectorySummary requires 'neutrasearch serve --index INDEX.nsx'".into(),
            ),
        )?;
        return Ok(());
    };
    let store = store.read().unwrap();
    if stale.load(Ordering::Acquire) {
        send(
            out,
            &HelperMsg::Error(
                "index became stale before the request; rebuild and restart the service".into(),
            ),
        )?;
        return Ok(());
    }
    match store.directory_summary(source, &path) {
        Ok(entry) => send(out, &HelperMsg::DirectorySummary { entry })?,
        Err(error) => send(out, &HelperMsg::Error(format!("{error:#}")))?,
    }

    Ok(())
}
