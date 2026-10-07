//! The GUI application: one concern per module.
//! - `types` — lane rows, transport events, persisted settings
//! - `state` — the field state and bootstrap
//! - `scans` — scan lifecycle and onboarding transitions
//! - `events` — the per-frame event pump
//! - `cache_events` — compact-index publication
//! - `scan_events` — scan-completion handling
//! - `queries` — search execution and tree-model preparation

mod cache_events;
mod events;
mod queries;
mod scan_events;
mod scans;
pub(crate) mod search_worker;
mod state;
mod types;

pub(crate) use state::NeutraApp;
pub(crate) use types::{Event, GuiSettings, LaneState};

impl NeutraApp {
    pub(crate) fn process_events(&mut self) -> bool {
        events::process_events(self)
    }

    pub(crate) fn add_root(&mut self, root: std::path::PathBuf) {
        scans::add_root(self, root)
    }

    pub(crate) fn begin_scan(&mut self) {
        scans::begin_scan(self)
    }

    pub(crate) fn begin_scan_with_elevation(&mut self, elevated: bool) {
        scans::begin_scan_with_elevation(self, elevated)
    }

    pub(crate) fn cancel_scan(&mut self) {
        scans::cancel_scan(self)
    }

    pub(crate) fn complete_onboarding_and_scan(&mut self) {
        scans::complete_onboarding_and_scan(self)
    }

    pub(crate) fn index_is_empty(&self) -> bool {
        queries::index_is_empty(self)
    }

    pub(crate) fn index_len(&self) -> u64 {
        queries::index_len(self)
    }

    /// Re-run the listing, except on the very first pass over a huge index
    /// (a full decode costs minutes of CPU); the first typed query starts it.
    pub(crate) fn requery_unless_huge(&mut self) {
        let first = self.search_seq == 0 && self.query.is_empty();
        if !(first && self.index_len() > crate::LAUNCH_LISTING_MAX && !queries::ranked_ready(self)) {
            self.requery();
        }
    }

    pub(crate) fn ensure_ranked(&mut self) {
        queries::ensure_ranked(self)
    }

    pub(crate) fn requery(&mut self) {
        queries::requery(self)
    }

    pub(crate) fn remove_root(&mut self, index: usize) {
        scans::remove_root(self, index)
    }

    pub(crate) fn request_tree_model(&mut self) {
        queries::request_tree_model(self)
    }

    pub(crate) fn ensure_tree_summary(&mut self) {
        queries::ensure_tree_summary(self)
    }

    pub(crate) fn save_settings(&mut self) {
        scans::save_settings(self)
    }

    pub(crate) fn scan_len(&self) -> u64 {
        queries::scan_len(self)
    }
}
