use super::{reject_index, CompactIndex, NeutraApp};

/// Notice live changes: four times a second, compare the delta file's
/// stamp and refresh results when it moved. Keep the previous stamp while a
/// search runs, so writes coalesce into one refresh after it finishes instead
/// of repeatedly cancelling work on a busy filesystem.
pub(crate) fn poll_delta(app: &mut NeutraApp) {
    if app.delta_checked.elapsed() < std::time::Duration::from_millis(250) {
        return;
    }
    app.delta_checked = std::time::Instant::now();
    let mut stale_path = app.cache_path.as_os_str().to_os_string();
    stale_path.push(".stale");
    if std::path::PathBuf::from(stale_path).is_file() {
        reject_index(
            app,
            "Native metadata updates failed; review index details and rebuild the index".into(),
        );
        app.tree_model = None;
        return;
    }
    if app.compact.is_none() && app.cache_path.is_file() {
        if let Ok(index) = CompactIndex::open_fast(&app.cache_path) {
            crate::app::cache_events::adopt_published_index(app, index);
            app.ensure_ranked();
            app.ensure_tree_summary();
            app.requery();
        }
    }
    let mut status_path = app.cache_path.as_os_str().to_os_string();
    status_path.push(".watch-status");
    if let Ok(file) = std::fs::File::open(std::path::PathBuf::from(status_path)) {
        use std::io::Read;
        let mut status = String::new();
        if file.take(8192).read_to_string(&mut status).is_ok() {
            if status.trim().is_empty() {
                app.lanes.remove("watch");
            } else {
                crate::app::events::note(app, "watch", "LIVE UPDATES", status, true);
            }
        }
    }
    let stamp = std::fs::metadata(app.cache_path.with_extension("delta"))
        .ok()
        .and_then(|meta| Some((meta.modified().ok()?, meta.len())));
    let changed = app.delta_stamp.is_some_and(|before| before != stamp);
    if changed {
        if app.searching {
            return;
        }
        app.delta_stamp = Some(stamp);
        app.last_query = None;
        app.requery();
    } else {
        app.delta_stamp = Some(stamp);
    }
}
