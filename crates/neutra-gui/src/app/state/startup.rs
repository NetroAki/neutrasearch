use super::super::EventSender;
use super::*;

impl NeutraApp {
    pub(super) fn finish_startup(
        &mut self,
        first_run: bool,
        has_durable_index: bool,
        cache_error: Option<String>,
        tx: EventSender,
    ) {
        self.seed_lanes(has_durable_index, cache_error, first_run);
        // Settings files written by older versions may still record an
        // unfinished setup run; the folder question is gone, so complete
        // setup here and let a failed scan surface through the access
        // banner instead.
        if !self.onboarding_complete {
            self.onboarding_complete = true;
            self.save_settings();
        }
        self.requery_unless_huge();
        self.ensure_ranked();
        self.ensure_tree_summary();
        if env_flag("NEUTRASEARCH_AUTO_PROVISION_REMOTE") {
            crate::transport::spawn_network_watcher(tx);
            self.remote_watcher_started = true;
        }
        if self.index_is_empty() {
            // A missing or empty index is never a valid idle first screen. This
            // also repairs partial installs that wrote settings before their
            // first usable scan completed.
            self.selected_roots = default_system_roots();
            self.onboarding_scan = true;
            self.save_settings();
            self.begin_scan_with_elevation(cfg!(target_os = "linux"));
        }
        super::super::indexing_lifecycle::resume(self);
    }

    fn seed_lanes(
        &mut self,
        has_durable_index: bool,
        cache_error: Option<String>,
        first_run: bool,
    ) {
        if has_durable_index {
            let records = self.index_len();
            self.lanes.insert(
                "cache".into(),
                LaneState {
                    label: "DURABLE INDEX".into(),
                    status: format!("restored {records} entries"),
                    records,
                    ..LaneState::default()
                },
            );
            return;
        }
        if let Some(error) = cache_error {
            self.lanes.insert(
                "cache-error".into(),
                LaneState {
                    label: "INDEX ERROR".into(),
                    status: error.clone(),
                    error: true,
                    ..LaneState::default()
                },
            );
            return;
        }
        self.lanes.insert(
            "welcome".into(),
            LaneState {
                label: "READY".into(),
                status: if first_run {
                    "Scanning all local system drives automatically".into()
                } else {
                    "Choose Scan to build the local index".into()
                },
                ..LaneState::default()
            },
        );
    }
}
