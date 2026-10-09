//! The framed protocol loop: Hello handshake, optional client
//! authentication, command dispatch, and the watch-thread lifecycle.
//! Split from `main` so entry points stay separate from wire handling.

pub(crate) use crate::scan::ProtocolOutput;
use crate::scan::{
    launch_scans, prepare_scan, reap_scan_threads, validate_delta_changes, validate_query,
};
use crate::store::{write_stale_marker, DurableStore};
#[cfg(target_os = "linux")]
use crate::{watch_linux, watch_mount, MAX_PENDING_CHANGES, WATCH_DEBOUNCE};
use anyhow::{Context, Result};
use neutra_core::proto::{
    read_frame, write_frame, ClientMsg, HelperMsg, HELPER_BUILD, PROTO_VERSION,
};
use neutra_core::{DeltaChange, Index};
use std::collections::BTreeMap;
use std::io::{BufWriter, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

pub(crate) fn run_protocol<R: Read>(
    rin: &mut R,
    writer: Box<dyn Write + Send>,
    serve_index: Option<std::path::PathBuf>,
    watch_mount: Option<(std::path::PathBuf, u32)>,
    stop_requested: Option<&AtomicBool>,
) -> Result<()> {
    run_protocol_with_auth(rin, writer, serve_index, watch_mount, stop_requested, None)
}

pub fn run_protocol_with_auth<R: Read>(
    rin: &mut R,
    writer: Box<dyn Write + Send>,
    serve_index: Option<std::path::PathBuf>,
    watch_mount: Option<(std::path::PathBuf, u32)>,
    stop_requested: Option<&AtomicBool>,
    authenticate: Option<&dyn Fn() -> Result<()>>,
) -> Result<()> {
    let out: ProtocolOutput = Arc::new(Mutex::new(BufWriter::new(writer)));

    tracing::info!(target: "neutra_helper::protocol", "waiting for client Hello");
    // Expect Hello first.
    let hello: Option<ClientMsg> = read_frame(rin).context("reading Hello")?;
    tracing::info!(target: "neutra_helper::protocol", received = hello.is_some(), "client Hello read");
    match hello {
        Some(ClientMsg::Hello { proto }) if proto == PROTO_VERSION => {
            send(
                &out,
                &HelperMsg::Hello {
                    proto: PROTO_VERSION,
                    build: HELPER_BUILD,
                    os: std::env::consts::OS.to_string(),
                    arch: std::env::consts::ARCH.to_string(),
                },
            )?;
            tracing::info!(target: "neutra_helper::protocol", "helper Hello sent; starting client authentication");
            // The client needs the Hello response before image verification can
            // complete on Windows. Authentication still happens before any
            // command is read or executed.
            if let Some(authenticate) = authenticate {
                authenticate().context("authenticate pipe client")?;
                tracing::info!(target: "neutra_helper::protocol", "client authentication completed");
            }
            tracing::info!(target: "neutra_helper::protocol", "authentication boundary passed; entering command loop");
        }
        Some(ClientMsg::Hello { proto }) => {
            send(
                &out,
                &HelperMsg::Error(format!(
                    "protocol mismatch: client={proto} helper={PROTO_VERSION}"
                )),
            )?;
            return Ok(());
        }
        _ => {
            send(&out, &HelperMsg::Error("expected Hello".into()))?;
            return Ok(());
        }
    }

    // Scans populate one resident index; searches never trigger a rescan.
    let index = Arc::new(RwLock::new(Index::default()));
    #[cfg(target_os = "linux")]
    let watches = watch_mount
        .as_ref()
        .map(|(mount, source)| {
            crate::watch_session::WatchSession::prepare(
                serve_index
                    .as_ref()
                    .context("watch mode requires an index")?,
                mount,
                *source,
            )
        })
        .transpose()?;
    let durable = serve_index
        .as_ref()
        .map(|path| DurableStore::open(path).map(|store| Arc::new(RwLock::new(store))))
        .transpose()?;
    let stale = Arc::new(AtomicBool::new(false));
    #[cfg(target_os = "linux")]
    if let Some(watches) = watches {
        let store = Arc::clone(durable.as_ref().expect("watch mode has a durable store"));
        watches.start(store, Arc::clone(&stale))?;
        anyhow::ensure!(
            !stale.load(Ordering::Acquire),
            "native watcher failed during startup"
        );
        send(&out, &HelperMsg::WatchReady)?;
    }
    #[cfg(not(target_os = "linux"))]
    if watch_mount.is_some() {
        anyhow::bail!(
            "native watch mode is not implemented on {}",
            std::env::consts::OS
        );
    }
    let mut scan_threads = Vec::new();
    tracing::info!(target: "neutra_helper::protocol", "waiting for command frame");
    loop {
        let frame = read_frame(rin);
        if stop_requested.is_some_and(|stop| stop.load(Ordering::Acquire)) {
            // Windows service shutdown disconnects the pipe. Return without
            // joining read-only scan workers; the single-service process exits
            // immediately and the GUI discards its unpublished staging index.
            return Ok(());
        }
        let msg: Option<ClientMsg> = frame.context("reading command")?;
        tracing::info!(target: "neutra_helper::protocol", received = msg.is_some(), "command frame read");
        reap_scan_threads(&mut scan_threads);
        match msg {
            None | Some(ClientMsg::Shutdown) => break,
            Some(ClientMsg::Hello { .. }) => {
                send(&out, &HelperMsg::Error("duplicate Hello".into()))?;
            }
            Some(ClientMsg::Scan {
                mounts,
                roots,
                allow_zfs_enumerate,
            }) => {
                tracing::info!(target: "neutra_helper::protocol", "Scan command dispatch");
                if allow_zfs_enumerate {
                    send(
                        &out,
                        &HelperMsg::Error(
                            "Directory walking is disabled; ZFS needs a native metadata backend"
                                .into(),
                        ),
                    )?;
                    continue;
                }
                match prepare_scan(mounts, roots, scan_threads.is_empty()) {
                    Ok((mounts, roots)) => {
                        launch_scans(mounts, roots, &out, None, &mut scan_threads)
                    }
                    Err(error) => {
                        send(&out, &HelperMsg::Error(error.to_string()))?;
                        if stop_requested.is_none() {
                            return Ok(());
                        }
                    }
                }
            }
            Some(ClientMsg::ScanResident { mounts, roots, .. }) => {
                match prepare_scan(mounts, roots, scan_threads.is_empty()) {
                    Ok((mounts, roots)) => {
                        launch_scans(mounts, roots, &out, Some(&index), &mut scan_threads)
                    }
                    Err(error) => {
                        send(&out, &HelperMsg::Error(error.to_string()))?;
                        if stop_requested.is_none() {
                            return Ok(());
                        }
                    }
                }
            }
            Some(ClientMsg::Search { query }) => {
                if stale.load(Ordering::Acquire) {
                    send(
                        &out,
                        &HelperMsg::Error(
                            "index is stale; run a full native reindex before searching".into(),
                        ),
                    )?;
                    continue;
                }
                if let Err(error) = validate_query(&query) {
                    send(&out, &HelperMsg::Error(error.to_string()))?;
                    continue;
                }
                let (hits, stats) = if let Some(store) = &durable {
                    let store = store.read().unwrap();
                    if stale.load(Ordering::Acquire) {
                        send(
                            &out,
                            &HelperMsg::Error(
                                "index became stale during the query; rebuild and restart the service"
                                    .into(),
                            ),
                        )?;
                        continue;
                    }
                    store.search(&query)?
                } else {
                    index.read().unwrap().search(&query)?
                };
                send(
                    &out,
                    &HelperMsg::SearchResult {
                        hits: hits.into_iter().map(|hit| hit.record).collect(),
                        wall_us: stats.wall_us,
                    },
                )?;
            }
            Some(ClientMsg::DirectorySummary { source, path }) => {
                let Some(store) = &durable else {
                    send(
                        &out,
                        &HelperMsg::Error(
                            "DirectorySummary requires 'neutrasearch serve --index INDEX.nsx'"
                                .into(),
                        ),
                    )?;
                    continue;
                };
                let store = store.read().unwrap();
                if stale.load(Ordering::Acquire) {
                    send(
                        &out,
                        &HelperMsg::Error(
                            "index became stale before the request; rebuild and restart the service"
                                .into(),
                        ),
                    )?;
                    continue;
                }
                match store.directory_summary(source, &path) {
                    Ok(entry) => send(&out, &HelperMsg::DirectorySummary { entry })?,
                    Err(error) => send(&out, &HelperMsg::Error(format!("{error:#}")))?,
                }
            }
            Some(ClientMsg::ApplyDelta { changes }) => {
                if let Err(error) = validate_delta_changes(&changes) {
                    send(&out, &HelperMsg::Error(error.to_string()))?;
                    continue;
                }
                if stale.load(Ordering::Acquire) {
                    send(
                        &out,
                        &HelperMsg::Error(
                            "index is stale; run a full native reindex before applying changes"
                                .into(),
                        ),
                    )?;
                    continue;
                }
                let Some(store) = &durable else {
                    send(
                        &out,
                        &HelperMsg::Error(
                            "ApplyDelta requires 'neutrasearch serve --index INDEX.nsx'".into(),
                        ),
                    )?;
                    continue;
                };
                let mut store = store.write().unwrap();
                if stale.load(Ordering::Acquire) {
                    send(
                        &out,
                        &HelperMsg::Error(
                            "index became stale before the update; rebuild and restart the service"
                                .into(),
                        ),
                    )?;
                    continue;
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
                            &out,
                            &HelperMsg::DeltaApplied {
                                changes: applied.changes,
                                wal_bytes: applied.wal_bytes,
                                needs_compaction: false,
                            },
                        )?;
                    }
                    Err(error) => {
                        stale.store(true, Ordering::Release);
                        if let Err(marker_error) =
                            write_stale_marker(&store.path, &error.to_string())
                        {
                            tracing::error!("failed to persist stale marker: {marker_error:#}");
                        }
                        send(
                            &out,
                            &HelperMsg::Error(format!(
                                "delta commit or compaction failed; index disabled until rebuild: {error:#}"
                            )),
                        )?;
                    }
                }
            }
        }
    }

    for t in scan_threads {
        let _ = t.join();
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub(crate) fn start_native_watch(
    mut watcher: Box<dyn watch_mount::Watch>,
    store: Arc<RwLock<DurableStore>>,
    stale: Arc<AtomicBool>,
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
            if let Err(error) = drain_watch_burst(watcher.as_mut(), &mut pending) {
                fail_closed(&stale, &base_path, error);
                break;
            }
            let changes = std::mem::take(&mut pending)
                .into_values()
                .collect::<Vec<_>>();
            let applied = match store.write() {
                Ok(mut store) => match store.apply_bounded(changes) {
                    Ok(applied) => Ok(applied),
                    Err(error) => {
                        // Publish the failure while the write lock is still
                        // held. Searches recheck stale after acquiring their
                        // read lock, so non-durable memory is never served.
                        stale.store(true, Ordering::Release);
                        Err(error)
                    }
                },
                Err(_) => {
                    stale.store(true, Ordering::Release);
                    Err(anyhow::anyhow!("durable store lock poisoned"))
                }
            };
            match applied {
                Ok(applied) => {
                    if let Some(compacted) = applied.compacted {
                        tracing::info!(
                            records = compacted.records,
                            bytes = compacted.bytes,
                            "compacted delta into replacement base"
                        );
                    }
                    tracing::debug!(
                        changes = applied.changes,
                        wal_bytes = applied.wal_bytes,
                        "native watch batch committed"
                    );
                }
                Err(error) => {
                    fail_closed(&stale, &base_path, error);
                    break;
                }
            }
        }
    });
}

#[cfg(target_os = "linux")]
fn drain_watch_burst(
    watcher: &mut dyn watch_mount::Watch,
    pending: &mut BTreeMap<String, DeltaChange>,
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
            watch_linux::WatchBatch::RescanRequired(reason) => anyhow::bail!(reason),
        }
    }
    Ok(())
}

fn merge_watched_changes(pending: &mut BTreeMap<String, DeltaChange>, changes: Vec<DeltaChange>) {
    for change in changes {
        let key = match &change {
            DeltaChange::Upsert(record) => record.path.to_string(),
            DeltaChange::Remove(path) => path.to_string(),
        };
        pending.insert(key, change);
    }
}

fn fail_closed(stale: &AtomicBool, base_path: &std::path::Path, error: anyhow::Error) {
    stale.store(true, Ordering::Release);
    if let Err(marker_error) = write_stale_marker(base_path, &error.to_string()) {
        tracing::error!("failed to persist stale marker: {marker_error:#}");
    }
    tracing::error!("native watch stopped: {error:#}");
}

fn send(out: &ProtocolOutput, msg: &HelperMsg) -> Result<()> {
    let mut w = out.lock().unwrap();
    write_frame(&mut *w, msg)?;
    Ok(())
}

pub(crate) fn send_lossy(out: &ProtocolOutput, msg: &HelperMsg) {
    if let Err(e) = send(out, msg) {
        tracing::warn!("failed to send frame: {e}");
    }
}

#[cfg(all(test, target_os = "linux"))]
mod watch_tests {
    use super::*;

    struct ContinuousWrites {
        reads: usize,
        overflow: bool,
    }

    impl watch_mount::Watch for ContinuousWrites {
        fn read_batch(&mut self) -> Result<watch_linux::WatchBatch> {
            self.reads += 1;
            if self.overflow {
                Ok(watch_linux::WatchBatch::RescanRequired(
                    "native queue overflow",
                ))
            } else {
                Ok(watch_linux::WatchBatch::Changes(vec![DeltaChange::Remove(
                    "/busy".into(),
                )]))
            }
        }

        fn wait_readable(&self, _timeout: std::time::Duration) -> std::io::Result<bool> {
            std::thread::sleep(std::time::Duration::from_millis(1));
            Ok(true)
        }
    }

    #[test]
    fn continuous_writes_publish_without_waiting_for_quiet() {
        let mut watcher = ContinuousWrites {
            reads: 0,
            overflow: false,
        };
        let mut pending = BTreeMap::new();
        let started = std::time::Instant::now();
        drain_watch_burst(&mut watcher, &mut pending).unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        assert!(watcher.reads > 0);
        assert_eq!(pending.len(), 1);
    }

    #[test]
    fn queue_overflow_is_reported_instead_of_serving_incomplete_updates() {
        let mut watcher = ContinuousWrites {
            reads: 0,
            overflow: true,
        };
        let error = drain_watch_burst(&mut watcher, &mut BTreeMap::new()).unwrap_err();
        assert!(error.to_string().contains("native queue overflow"));
    }
}
