//! Scan lifecycle: starting a helper scan (plain or elevated), and the
//! onboarding transitions around it.

use super::state::NeutraApp;
use super::types::GuiSettings;
use crate::{selected_scan_mounts, spawn_local_helper};

pub(crate) fn begin_scan(app: &mut NeutraApp) {
    begin_scan_with_elevation(app, false);
}

pub(crate) fn begin_scan_with_elevation(app: &mut NeutraApp, elevated: bool) {
    if app.scanning || app.building_cache {
        return;
    }
    if app.selected_roots.is_empty() {
        app.compact = None;
        app.index = neutra_core::Index::default();
        app.tree_model = None;
        app.tree_building = false;
        app.cache_dirty = true;
        app.last_cache = std::time::Instant::now() - std::time::Duration::from_secs(3);
        app.lanes.clear();
        app.lanes.insert(
            "locations".into(),
            super::types::LaneState {
                label: "SEARCH LOCATIONS".into(),
                status: "no folders selected".into(),
                ..super::types::LaneState::default()
            },
        );
        app.requery();
        return;
    }
    // Spill batches to disk as they arrive; the last complete index remains
    // searchable until the fresh build publishes.
    app.scan_index = neutra_core::SpillAccumulator::begin(&app.cache_path).ok();
    if app.scan_index.is_none() {
        app.scanning = false;
        app.scan_roots.clear();
        app.onboarding_scan = false;
        app.lanes.insert(
            "scan".into(),
            super::types::LaneState {
                label: "NATIVE SCAN".into(),
                status: "cannot spill scan batches; keeping the last complete index".into(),
                ..super::types::LaneState::default()
            },
        );
        return;
    }
    app.scan_roots = app.selected_roots.clone();
    app.scanning = true;
    app.active_scans = 0;
    app.cache_dirty = false;
    app.lanes.clear();
    let mounts = selected_scan_mounts(&app.selected_roots);
    if mounts.is_empty() {
        app.scanning = false;
        app.scan_index = None;
        app.scan_roots.clear();
        app.onboarding_scan = false;
        app.lanes.insert(
            "locations".into(),
            super::types::LaneState {
                label: "SEARCH LOCATIONS".into(),
                status: "selected folders are not on a supported local filesystem".into(),
                error: true,
                ..super::types::LaneState::default()
            },
        );
        return;
    }
    spawn_local_helper(
        app.tx.clone(),
        elevated,
        mounts,
        app.scan_roots.clone(),
        false,
    );
}

pub(crate) fn complete_onboarding_and_scan(app: &mut NeutraApp) {
    if app.selected_roots.is_empty() {
        return;
    }
    // Persist the chosen roots, but do not dismiss setup until at least one
    // requested native lane has produced a usable index.
    app.onboarding_complete = false;
    app.onboarding_scan = true;
    app.save_settings();
    app.begin_scan_with_elevation(cfg!(target_os = "linux"));
}

pub(crate) fn save_settings(app: &mut NeutraApp) {
    let settings = GuiSettings::current(app);
    if let Err(error) = crate::save_gui_settings(&app.settings_path, &settings) {
        app.lanes.insert(
            "settings".into(),
            super::types::LaneState {
                label: "SETTINGS".into(),
                status: error,
                error: true,
                ..super::types::LaneState::default()
            },
        );
    }
}

pub(crate) fn add_root(app: &mut NeutraApp, root: std::path::PathBuf) {
    let root = crate::normalize_selected_root(std::fs::canonicalize(&root).unwrap_or(root));
    if !root.is_absolute()
        || app
            .selected_roots
            .iter()
            .any(|existing| crate::same_root(existing, &root))
    {
        return;
    }
    app.selected_roots.push(root);
    app.selected_roots.sort();
    app.scope_root = None;
    app.setup_focus_requested = true;
    if app.onboarding_complete {
        app.save_settings();
        app.requery();
        app.begin_scan_with_elevation(cfg!(target_os = "linux"));
    }
}

pub(crate) fn remove_root(app: &mut NeutraApp, index: usize) {
    if index >= app.selected_roots.len() {
        return;
    }
    app.selected_roots.remove(index);
    app.scope_root = None;
    app.save_settings();
    app.requery();
    if app.onboarding_complete {
        app.begin_scan_with_elevation(cfg!(target_os = "linux"));
    }
}
