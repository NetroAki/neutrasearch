use super::*;

/// Aggregate lane state for one selected root: any failed lane reads
/// Unavailable, an unfinished scan reads Indexing, finished lanes Ready.
pub(super) fn root_status(app: &NeutraApp, root: &str) -> (&'static str, egui::Color32) {
    use crate::ui::icons::GREEN;
    use crate::ui::widgets::{ERROR, MUTED, VIOLET};
    let failed = app
        .lanes
        .iter()
        .any(|(key, lane)| lane.error && covers(key, root));
    if failed {
        return ("Unavailable", ERROR);
    }
    let known: Vec<&crate::LaneState> = app
        .lanes
        .iter()
        .filter(|(key, _)| covers(key, root))
        .map(|(_, lane)| lane)
        .collect();
    if known.is_empty() || !app.scanning {
        if app.index_is_empty() {
            return ("-", MUTED);
        }
        return ("Ready", GREEN);
    }
    if known.iter().all(|lane| lane.records > 0) {
        ("Ready", GREEN)
    } else {
        ("Indexing\u{2026}", VIOLET)
    }
}

/// A lane covers a root when its mountpoint is the root or an ancestor.
fn covers(key: &str, root: &str) -> bool {
    key.starts_with('/') && (key == "/" || key == root || root.starts_with(&format!("{key}/")))
}
