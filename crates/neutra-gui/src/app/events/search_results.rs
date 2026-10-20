use super::{note, NeutraApp};

pub(super) fn apply(app: &mut NeutraApp, id: u64, result: crate::app::search_worker::SearchResult) {
    if id != app.search_seq {
        return;
    }
    app.searching = false;
    // The last native notification may have arrived during this search.
    // Its retained delta stamp must be checked on this completion wakeup.
    app.delta_checked = std::time::Instant::now()
        .checked_sub(std::time::Duration::from_secs(1))
        .unwrap_or_else(std::time::Instant::now);
    match result {
        Ok((hits, stats)) => {
            app.lanes.remove("search");
            app.hits = hits;
            let order: Vec<_> = app
                .hits
                .iter()
                .map(|hit| hit.record.path.as_ref())
                .collect();
            app.selection.retain(&order);
            if app
                .selected
                .as_deref()
                .is_some_and(|path| !order.contains(&path))
            {
                app.selected = None;
            }
            app.search_stats = stats;
        }
        Err(error) => {
            app.hits.clear();
            app.search_stats = neutra_core::SearchStats::default();
            note(app, "search", "SEARCH", error, true);
        }
    }
}
