use super::{NeutraApp, RuntimeState};

pub(super) fn runtime_state(app: &NeutraApp) -> RuntimeState {
    if let Ok(forced) = std::env::var("NEUTRASEARCH_GUI_STATE") {
        match forced.to_ascii_lowercase().as_str() {
            "first-run" => return RuntimeState::FirstRun,
            "indexing" => {
                return if app.index_is_empty() {
                    RuntimeState::IndexingInitial
                } else {
                    RuntimeState::IndexingBackground
                }
            }
            "permission" => return RuntimeState::Permission,
            "stale" => return RuntimeState::Stale,
            _ => {}
        }
    }
    let has_results = !app.index_is_empty();
    // File action failures do not indicate missing scanner access.
    let stale = app
        .lanes
        .iter()
        .any(|(key, lane)| lane.error && (key.contains("index") || key.contains("cache")));
    let has_scanner_error = app.lanes.iter().any(|(key, lane)| {
        lane.error
            && (key.contains("index")
                || key.contains("cache")
                || key == "helper"
                || key == "host"
                || key == "protocol"
                || key.contains("scan")
                || key.starts_with("remote:"))
    });
    if app.scanning || app.building_cache {
        if has_results {
            RuntimeState::IndexingBackground
        } else {
            RuntimeState::IndexingInitial
        }
    } else if stale && has_results {
        RuntimeState::Stale
    } else if !app.onboarding_complete {
        RuntimeState::FirstRun
    } else if has_scanner_error {
        RuntimeState::Permission
    } else {
        RuntimeState::Ready
    }
}
