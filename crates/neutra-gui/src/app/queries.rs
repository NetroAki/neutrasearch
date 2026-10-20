//! Queries: the resident/compact search path, tree-model preparation, and
//! the small accessors the UI reads per frame.

use super::state::NeutraApp;
use super::types::Event;
use neutra_core::CompactIndex;
mod live;
#[cfg(test)]
mod tests;
mod view;
pub(crate) use live::poll_delta;
use view::fetch_tree_dirs;
pub(crate) use view::requery;

pub(crate) fn index_len(app: &NeutraApp) -> u64 {
    app.compact
        .as_ref()
        .map_or(app.index.len() as u64, |index| index.len())
}

pub(crate) fn index_is_empty(app: &NeutraApp) -> bool {
    index_len(app) == 0
}

pub(crate) fn scan_len(app: &NeutraApp) -> u64 {
    app.scan_index.as_ref().map_or(0, |index| index.len())
}

pub(crate) fn data_generation(app: &NeutraApp) -> u64 {
    app.compact
        .as_ref()
        .map_or_else(|| app.index.generation(), |index| index.generation())
}

/// Fetch whatever the visible set lacks: root, the current chain, expanded
/// branches. Each fetch streams its subtrees with pages released, so tree
/// browsing holds megabytes, not the tens of gigabytes of full builds.
pub(crate) fn request_tree_model(app: &mut NeutraApp) {
    if index_is_empty(app) || app.tree_summary_pending {
        return;
    }
    let missing: Vec<String> = crate::ui::Hierarchy::missing_dirs(
        app.tree_model.as_ref(),
        &app.treemap_path,
        &app.tree_expanded,
    )
    .into_iter()
    .filter(|path| !app.tree_pending.contains(path))
    .collect();
    if missing.is_empty() {
        return;
    }
    app.tree_building = true;
    app.tree_pending.extend(missing.iter().cloned());
    let generation = data_generation(app);
    let tx = app.tx.clone();
    let current = app.treemap_path.clone();
    let compact_path = app.compact.as_ref().map(|_| app.cache_path.clone());
    let resident_records = compact_path.is_none().then(|| app.index.records().to_vec());
    std::thread::spawn(move || {
        let model = fetch_tree_dirs(
            &missing,
            &current,
            compact_path,
            generation,
            resident_records,
        );
        match model {
            Ok(model) => {
                let _ = tx.send(Event::TreeReady { generation, model });
            }
            Err(error) => {
                let _ = tx.send(Event::TreeFailed(error));
            }
        }
    });
}

pub(crate) fn ranked_ready(app: &NeutraApp) -> bool {
    app.compact.as_ref().is_some_and(|compact| {
        neutra_core::BrowserIndex::is_current(&app.cache_path, compact.generation())
    })
}

/// Build the sorted leader lists once per index generation so newest-first
/// and largest-first listings never decode the whole index.
pub(crate) fn ensure_ranked(app: &mut NeutraApp) {
    let Some(compact) = &app.compact else { return };
    if app.rank_pending || ranked_ready(app) {
        return;
    }
    let generation = compact.generation();
    let path = app.cache_path.clone();
    let tx = app.tx.clone();
    app.rank_pending = true;
    std::thread::spawn(move || {
        let ok = neutra_core::BrowserIndex::ensure(&path, generation).is_ok();
        let _ = tx.send(Event::Ranked { generation, ok });
    });
}

/// Build the shallow folder summary in the background so the tree opens
/// instantly. The tree waits for it instead of scanning the whole index.
pub(crate) fn ensure_tree_summary(app: &mut NeutraApp) {
    let Some(compact) = &app.compact else { return };
    if app.tree_summary_pending {
        return;
    }
    let generation = compact.generation();
    let path = app.cache_path.clone();
    let tx = app.tx.clone();
    app.tree_summary_pending = true;
    app.tree_building = true;
    std::thread::spawn(move || {
        let ok = neutra_core::TreeSummary::ensure(&path, generation).is_ok();
        let _ = tx.send(Event::TreeSummary { generation, ok });
    });
}

#[cfg(test)]
use view::apply_kind_filter;

fn reject_index(app: &mut NeutraApp, status: String) {
    app.hits.clear();
    app.lanes.insert(
        "index".into(),
        super::types::LaneState {
            label: "Durable index".into(),
            status,
            error: true,
            ..super::types::LaneState::default()
        },
    );
}
