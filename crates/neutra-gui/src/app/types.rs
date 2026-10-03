//! Shared app types: lane status rows, transport events, and persisted
//! settings.

use neutra_core::proto::HelperMsg;
use neutra_core::CompactIndex;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::ui;

#[derive(Default, Clone)]
pub(crate) struct LaneState {
    pub(crate) label: String,
    pub(crate) status: String,
    pub(crate) records: u64,
    pub(crate) ms: u64,
    pub(crate) error: bool,
}

pub(crate) enum Event {
    Message(HelperMsg),
    Fatal(String),
    Remote {
        key: String,
        status: String,
        error: bool,
    },
    CompactReady(CompactIndex),
    CompactFailed(String),
    TreeReady {
        generation: u64,
        model: ui::Hierarchy,
    },
    TreeFailed(String),
    SearchDone {
        id: u64,
        result: super::search_worker::SearchResult,
    },
    TreeSummary {
        generation: u64,
        ok: bool,
    },
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct GuiSettings {
    pub(crate) onboarding_complete: bool,
    pub(crate) roots: Vec<PathBuf>,
    #[serde(default)]
    pub(crate) view: ui::ResultView,
    pub(crate) sort_mode: ui::SortMode,
    #[serde(default)]
    pub(crate) sort_reversed: bool,
    #[serde(default)]
    pub(crate) search_mode: ui::SearchMode,
    #[serde(default)]
    pub(crate) case_sensitive: bool,
    #[serde(default)]
    pub(crate) regex_mode: bool,
    /// Per-mount record totals from the last completed scan: the honest
    /// denominator for progress percentages (native lanes never know the
    /// total upfront).
    #[serde(default)]
    pub(crate) last_mount_totals: std::collections::BTreeMap<String, u64>,
    /// Match whole words only (`call` skips `calling`).
    #[serde(default)]
    pub(crate) whole_word: bool,
    /// Fold accents before comparing (`cafe` finds `caf\u{e9}`).
    #[serde(default)]
    pub(crate) ignore_accents: bool,
}

impl GuiSettings {
    /// Snapshot of the user-visible choices, for persistence.
    pub(crate) fn current(app: &super::state::NeutraApp) -> Self {
        Self {
            onboarding_complete: app.onboarding_complete,
            roots: app.selected_roots.clone(),
            view: app.view_mode,
            sort_mode: app.sort_mode,
            sort_reversed: app.sort_reversed,
            search_mode: app.search_mode,
            case_sensitive: app.case_sensitive,
            regex_mode: app.regex_mode,
            last_mount_totals: app.mount_totals.clone(),
            whole_word: app.whole_word,
            ignore_accents: app.ignore_accents,
        }
    }
}
