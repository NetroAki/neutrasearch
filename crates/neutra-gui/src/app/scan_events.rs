//! Scan-lifecycle events: the transition from a finished helper session to
//! (possibly) a new resident index.

use super::events::note;
use super::state::NeutraApp;

pub(crate) fn handle_scan_complete(app: &mut NeutraApp, mounts: u32, errors: u32) {
    app.scanning = false;
    app.active_scans = 0;
    // A completion that arrives after Cancel (helper finished between click
    // and kill) adopts nothing: staging was dropped at click time.
    if app.cancelling {
        app.cancelling = false;
        app.scan_index = None;
        app.scan_roots.clear();
        end_scan_setup(app);
        note(
            app,
            "scan",
            "NATIVE SCAN",
            "Indexing cancelled, previous index kept",
            false,
        );
        return;
    }
    let staging = app.scan_index.take();
    app.scan_roots.clear();
    if mounts == 0 {
        end_scan_setup(app);
        let os = std::env::consts::OS;
        note(
            app,
            "scan",
            "NATIVE SCAN",
            format!("no supported native filesystems were discovered on {os}"),
            true,
        );
    } else if crate::transport::scan_has_reachable_lane(mounts, errors) {
        adopt_staging(app, staging, errors);
    } else {
        end_scan_setup(app);
        note(
            app,
            "scan",
            "NO REACHABLE LOCATIONS",
            format!("all {errors} unavailable native lane(s) were skipped; keeping the last complete index"),
            true,
        );
    }
    // Persist per-mount totals gathered during ScanDone for progress %.
    app.save_settings();
}

fn adopt_staging(app: &mut NeutraApp, spill: Option<neutra_core::SpillAccumulator>, errors: u32) {
    let Some(spill) = spill else { return };
    let runs = match spill.finish() {
        Ok(runs) => runs,
        Err(error) => {
            end_scan_setup(app);
            note(app, "scan", "NATIVE SCAN", error.to_string(), true);
            return;
        }
    };
    if runs.is_empty() {
        end_scan_setup(app);
        note(
            app,
            "scan",
            "EMPTY INDEX",
            "the native scanner returned no files; the previous index was kept",
            true,
        );
        return;
    }
    if app.onboarding_scan || !app.onboarding_complete {
        app.onboarding_complete = true;
        app.onboarding_scan = false;
        app.save_settings();
    }
    if errors > 0 {
        note(
            app,
            "scan",
            "PARTIAL INDEX",
            format!("indexed reachable locations; skipped {errors} unavailable native lane(s)"),
            false,
        );
    }
    // The previous base stays searchable while the fresh one builds; the
    // spill directory vanishes with the finished runs.
    super::cache_events::build_streaming_cache_index(app, runs);
}

/// Clear the one-shot onboarding state after any scan attempt.
pub(super) fn end_scan_setup(app: &mut NeutraApp) {
    app.onboarding_scan = false;
    app.setup_focus_requested = true;
}
