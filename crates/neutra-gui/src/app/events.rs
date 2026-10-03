//! Event pump: batches transport events into app state per frame.

use super::state::NeutraApp;
use super::types::{Event, LaneState};
use neutra_core::proto::HelperMsg;
use std::time::Duration;

const MAX_EVENTS_PER_FRAME: usize = 64;
const CACHE_BUILD_DELAY: Duration = Duration::from_secs(2);

pub(crate) fn process_events(app: &mut NeutraApp) -> bool {
    let mut processed = 0;
    while processed < MAX_EVENTS_PER_FRAME {
        let Ok(event) = app.rx.try_recv() else { break };
        processed += 1;
        handle_event(app, event);
    }
    let generation = crate::app::queries::data_generation(app);
    if generation != app.last_generation {
        app.last_generation = generation;
        app.tree_model = None;
        app.requery();
        app.ensure_tree_summary();
    }
    if app.cache_dirty
        && !app.building_cache
        && app.active_scans == 0
        && app.last_cache.elapsed() > CACHE_BUILD_DELAY
    {
        super::cache_events::rebuild_cache_index(app);
    }
    processed == MAX_EVENTS_PER_FRAME
}

fn handle_event(app: &mut NeutraApp, event: Event) {
    match event {
        Event::Message(msg) => handle_helper_message(app, msg),
        Event::Fatal(error) => {
            // A kill after Cancel surfaces as the helper stopping early;
            // report it as a deliberate cancel, not a crash.
            if app.cancelling && error.contains("stopped before completing") {
                end_scan(app);
                app.cancelling = false;
                note(app, "scan", "NATIVE SCAN", "Indexing cancelled, previous index kept", false);
                app.requery();
            } else {
                end_scan(app);
                note(app, "helper", "HELPER", error, true);
            }
        }
        Event::Remote { key, status, error } => {
            let lane_key = format!("remote:{key}");
            let label = format!("REMOTE/{key}");
            note(app, lane_key, label, status, error);
        }
        Event::CompactReady(index) => {
            super::cache_events::adopt_published_index(app, index);
        }
        Event::CompactFailed(error) => {
            // Keep serving the complete resident index when publication fails.
            app.building_cache = false;
            app.cache_dirty = false;
            note(app, "cache", "INDEX BUILD", error, true);
            app.requery();
        }
         Event::TreeReady { generation, model } => {
             app.tree_building = false;
             app.tree_pending.clear();
             if generation == crate::app::queries::data_generation(app) {
                 let pinned =
                     crate::ui::Hierarchy::want_dirs(&app.treemap_path, &app.tree_expanded);
                 match app.tree_model.as_mut() {
                     Some(hierarchy) => hierarchy.merge(model, &app.treemap_path, &pinned),
                     None => app.tree_model = Some(model),
                 }
             }
         }
        Event::TreeFailed(error) => {
            app.tree_building = false;
            note(app, "tree", "DISK MAP", error, true);
        }
        Event::SearchDone { id, result } => {
            // Only the newest search may update the view.
            if id == app.search_seq {
                app.searching = false;
                if let Some((hits, stats)) = result {
                    app.hits = hits;
                    app.search_stats = stats;
                }
            }
        }
        Event::TreeSummary { generation, ok } => {
            app.tree_summary_pending = false;
            app.tree_building = false;
            if !ok {
                note(app, "tree", "FOLDER MAP", "Folder map unavailable; folders load directly", false);
            } else if generation != crate::app::queries::data_generation(app) {
                app.ensure_tree_summary();
            }
        }
    }
}

fn handle_helper_message(app: &mut NeutraApp, msg: HelperMsg) {
    match msg {
        HelperMsg::Hello { os, arch, .. } => {
            let label = format!("{os}/{arch}");
            note(app, "host", label, "native helper online", false);
        }
        HelperMsg::ScanBegin { mount } => {
            app.active_scans += 1;
            let key = mount.mountpoint.display().to_string();
            let label = mount.fs.label().to_uppercase();
            let status = format!("indexing {key}");
            note(app, key, label, status, false);
        }
        HelperMsg::Records(records) => {
            let roots = app.scan_roots.clone();
            let batch = records
                .into_iter()
                .filter(|record| crate::record_in_roots(record.path.as_ref(), &roots))
                .collect::<Vec<_>>();
            count_staged(app, &batch);
            if let Some(spill) = &mut app.scan_index {
                if let Err(error) = spill.push_batch(batch) {
                    end_scan(app);
                    note(app, "scan", "NATIVE SCAN", error.to_string(), true);
                }
            }
        }
        HelperMsg::ScanDone { mount, stats } => {
            app.active_scans = app.active_scans.saturating_sub(1);
            // Post-cancel stragglers must not repaint finished lanes; the
            // completion path below reports the cancel exactly once.
            if !app.cancelling {
                let key = mount.mountpoint.display().to_string();
                let label = mount.fs.label().to_uppercase();
                let records = stats.records;
                let ms = stats.wall_ms;
                note(app, &key, label, stats.detail, false);
                if let Some(lane) = app.lanes.get_mut(&key) {
                    lane.records = records;
                    lane.ms = ms;
                }
                // Feeds progress percentages on later scans.
                if records > 0 {
                    app.mount_totals.insert(key, records);
                }
            }
        }
        HelperMsg::ScanError { mount, error } => {
            app.active_scans = app.active_scans.saturating_sub(1);
            let key = mount.mountpoint.display().to_string();
            let label = mount.fs.label().to_uppercase();
            note(app, key, label, error, true);
        }
        HelperMsg::ScanComplete { mounts, errors } => {
            super::scan_events::handle_scan_complete(app, mounts, errors);
        }
        HelperMsg::Error(error) => {
            end_scan(app);
            note(app, "protocol", "PROTOCOL", error, true);
        }
        HelperMsg::SearchResult { .. } => {}
        // Directory summaries are served to serve-mode clients; the GUI's
        // one-shot scan sessions never issue them.
        HelperMsg::DirectorySummary { .. } => {}
        HelperMsg::DeltaApplied {
            changes,
            wal_bytes,
            needs_compaction,
        } => {
            let suffix = if needs_compaction { " · compaction due" } else { "" };
            let status = format!("{changes} changes · {wal_bytes} bytes{suffix}");
            note(app, "delta", "LIVE DELTA", status, false);
        }
    }
}

pub(super) fn note(
    app: &mut NeutraApp,
    key: impl Into<String>,
    label: impl Into<String>,
    status: impl Into<String>,
    error: bool,
) {
    app.lanes.insert(
        key.into(),
        LaneState {
            label: label.into(),
            status: status.into(),
            error,
            ..LaneState::default()
        },
    );
}

/// Attribute a filtered batch to mounts by longest mountpoint prefix, for
/// live per-drive progress. Runs in the event pump, amortized per batch.
fn count_staged(app: &mut NeutraApp, batch: &[neutra_core::FileRecord]) {
    if batch.is_empty() {
        return;
    }
    let mounts: Vec<String> = app
        .lanes
        .keys()
        .filter(|key| key.starts_with('/'))
        .cloned()
        .collect();
    for record in batch {
        if let Some(mount) = longest_mount(&mounts, record.path.as_ref()) {
            *app.staged_by_mount.entry(mount).or_default() += 1;
        }
    }
}

fn longest_mount(mounts: &[String], path: &str) -> Option<String> {
    mounts
        .iter()
        .filter(|mount| {
            mount.as_str() == "/" || path == mount.as_str() || path.starts_with(&format!("{mount}/"))
        })
        .max_by_key(|mount| mount.len())
        .cloned()
}

fn end_scan(app: &mut NeutraApp) {
    app.scanning = false;
    app.active_scans = 0;
    app.scan_index = None;
    app.scan_roots.clear();
    super::scan_events::end_scan_setup(app);
}
