//! The framed protocol loop: Hello handshake, optional client
//! authentication, command dispatch, and the watch-thread lifecycle.
//! Split from `main` so entry points stay separate from wire handling.

use crate::scan::{
    launch_scans, prepare_scan, reap_scan_threads, validate_delta_changes, validate_query,
    watch_exclusions,
};
pub(crate) use crate::scan::ProtocolOutput;
#[cfg(target_os = "linux")]
use crate::{watch_linux, watch_mount, MAX_PENDING_CHANGES, WATCH_DEBOUNCE};
use crate::store::{write_stale_marker, DurableStore};
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
    let durable = serve_index
        .as_ref()
        .map(|path| DurableStore::open(path).map(|store| Arc::new(RwLock::new(store))))
        .transpose()?;
    let stale = Arc::new(AtomicBool::new(false));
    #[cfg(target_os = "linux")]
    if let Some((mountpoint, source)) = watch_mount {
        let mount = neutra_core::mounts::system_mounts()?
            .into_iter()
            .find(|mount| mount.mountpoint == mountpoint)
            .with_context(|| format!("no supported mount at {}", mountpoint.display()))?;
        let base_path = serve_index.as_ref().expect("watch mode has an index");
        let excluded = watch_exclusions(base_path);
        let store = Arc::clone(durable.as_ref().expect("watch mode has a durable store"));
        crate::watch_sweep::start(Arc::clone(&store), Arc::clone(&stale), source);
        match watch_linux::FanotifyWatcher::open(mount.clone(), source, excluded.clone()) {
            Ok(watcher) => start_native_watch(watcher, store, Arc::clone(&stale)),
            Err(error) if watch_mount::needs_mount_mark(&error) => {
                tracing::info!("subvolume mount: watching closed writes only");
                let watcher = watch_mount::MountWatcher::open(mount, source, excluded)?;
                start_native_watch(watcher, store, Arc::clone(&stale));
            }
            Err(error) => return Err(error),
        }
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
                    // The ZFS lane's walk gate reads this; a scan parameter
                    // survives pkexec where the environment does not.
                    std::env::set_var("NEUTRASEARCH_ZFS_ALLOW_WALK", "1");
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
    mut watcher: impl watch_mount::Watch,
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
        let mut rescan_seen = false;
        'watch: loop {
            match watcher.read_batch() {
                Ok(watch_linux::WatchBatch::Changes(changes)) => {
                    merge_watched_changes(&mut pending, changes);
                }
                Ok(watch_linux::WatchBatch::RescanRequired(reason)) => {
                    // A renamed directory or a queue overflow makes part of
                    // the tree ambiguous. The tracked directory itself is
                    // still delta-applied; the rest of the index keeps
                    // serving instead of failing every search until the
                    // next full reindex.
                    if !rescan_seen {
                        rescan_seen = true;
                        tracing::warn!(
                            reason,
                            "watched tree changed ambiguously; results may be incomplete until the next full reindex"
                        );
                    }
                }
                Err(error) => {
                    fail_closed(&stale, &base_path, error);
                    break;
                }
            }
            if pending.is_empty() {
                continue;
            }
            // Coalesce bursts: keep draining until 250ms of quiet or the
            // change cap, then commit once instead of fsync-per-event.
            while pending.len() < MAX_PENDING_CHANGES {
                match watcher.wait_readable(WATCH_DEBOUNCE) {
                    Ok(false) => break,
                    Ok(true) => match watcher.read_batch() {
                        Ok(watch_linux::WatchBatch::Changes(changes)) => {
                            merge_watched_changes(&mut pending, changes);
                        }
                        Ok(watch_linux::WatchBatch::RescanRequired(_)) => {
                            if !rescan_seen {
                                rescan_seen = true;
                                tracing::warn!(
                                    "watched tree changed ambiguously; results may be incomplete until the next full reindex"
                                );
                            }
                        }
                    Err(error) => {
                        fail_closed(&stale, &base_path, error);
                        break 'watch;
                    }
                },
                Err(error) => {
                    fail_closed(&stale, &base_path, error.into());
                    break 'watch;
                }
                }
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

/// Merge watched events into the pending set keyed by path; the most recent
/// change per path wins.
fn merge_watched_changes(
    pending: &mut BTreeMap<String, DeltaChange>,
    changes: Vec<DeltaChange>,
) {
    for change in changes {
        let key = match &change {
            DeltaChange::Upsert(record) => record.path.to_string(),
            DeltaChange::Remove(path) => path.to_string(),
        };
        pending.insert(key, change);
    }
}

/// Disable the index for a hard watch/commit failure: the fail-closed path
/// for errors that make the on-disk state untrustworthy.
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
