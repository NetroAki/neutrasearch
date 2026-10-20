use super::*;

pub(super) enum Flow {
    Continue,
    Shutdown,
    Disconnect,
}

pub(super) fn handle(
    msg: Option<ClientMsg>,
    session: &mut state::Session,
    service_mode: bool,
) -> Result<Flow> {
    let state::Session {
        out,
        index,
        durable,
        stale,
        scan_threads,
    } = session;
    match msg {
        None | Some(ClientMsg::Shutdown) => return Ok(Flow::Shutdown),
        Some(ClientMsg::Hello { .. }) => {
            send(out, &HelperMsg::Error("duplicate Hello".into()))?;
        }
        Some(ClientMsg::Scan {
            mounts,
            roots,
            allow_zfs_enumerate,
        }) => {
            tracing::info!(target: "neutra_helper::protocol", "Scan command dispatch");
            if allow_zfs_enumerate {
                send(
                    out,
                    &HelperMsg::Error(
                        "Directory walking is disabled; ZFS needs a native metadata backend".into(),
                    ),
                )?;
                return Ok(Flow::Continue);
            }
            match prepare_scan(mounts, roots, scan_threads.is_empty()) {
                Ok((mounts, roots)) => launch_scans(mounts, roots, out, None, scan_threads),
                Err(error) => {
                    send(out, &HelperMsg::Error(error.to_string()))?;
                    if !service_mode {
                        return Ok(Flow::Disconnect);
                    }
                }
            }
        }
        Some(ClientMsg::ScanResident { mounts, roots, .. }) => {
            match prepare_scan(mounts, roots, scan_threads.is_empty()) {
                Ok((mounts, roots)) => launch_scans(mounts, roots, out, Some(index), scan_threads),
                Err(error) => {
                    send(out, &HelperMsg::Error(error.to_string()))?;
                    if !service_mode {
                        return Ok(Flow::Disconnect);
                    }
                }
            }
        }
        Some(ClientMsg::Search { query }) => {
            search::handle(query, index, durable, stale, out)?;
        }
        Some(ClientMsg::DirectorySummary { source, path }) => {
            directory::handle(source, path, durable, stale, out)?;
        }
        Some(ClientMsg::ApplyDelta { changes }) => {
            delta::handle(changes, durable, stale, out)?;
        }
    }
    Ok(Flow::Continue)
}
