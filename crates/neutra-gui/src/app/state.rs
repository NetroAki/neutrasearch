//! Application field state and bootstrap. The struct holds everything the
//! UI and the transports touch; `new` wires defaults, restores the durable
//! index, and kicks off the first-run scan.

use super::types::{Event, GuiSettings, LaneState};
use crate::{
    compact_cache_path, default_system_roots, embedded_logo, env_flag, gui_settings_path,
    legacy_cache_path, load_gui_settings,
};
use neutra_core::{CompactIndex, Index, SearchHit, SearchStats, SpillAccumulator};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::Instant;

use eframe::egui;

use crate::ui;

pub(crate) struct NeutraApp {
    pub(crate) index: Index,
    pub(crate) compact: Option<std::sync::Arc<CompactIndex>>,
    pub(crate) logo: egui::TextureHandle,
    pub(crate) scan_index: Option<SpillAccumulator>,
    /// A lane failed during this scan; all staged batches must be discarded.
    pub(crate) scan_failed: bool,
    pub(crate) scan_cursors: Vec<neutra_core::NativeCursor>,
    pub(crate) query: String,
    pub(crate) hits: Vec<SearchHit>,
    pub(crate) search_stats: SearchStats,
    pub(crate) page_offset: usize,
    pub(crate) page_cursors: Vec<Option<neutra_core::FileRecord>>,
    pub(crate) last_query: Option<neutra_core::Query>,
    pub(crate) search_roots: Vec<PathBuf>,
    pub(crate) search_excluded: Vec<PathBuf>,
    /// Newest queued search id and whether its results are still pending.
    pub(crate) search_seq: u64,
    pub(crate) searching: bool,
    pub(crate) search_tx: super::search_worker::SearchQueue,
    /// Compiled text matcher for the current query, reused by result
    /// highlighting so a regex compiles once per query, not per row per frame.
    pub(crate) matcher: Option<neutra_core::QueryMatcher>,
    pub(crate) sort_reversed: bool,
    pub(crate) lanes: BTreeMap<String, LaneState>,
    pub(crate) rx: Receiver<Event>,
    pub(crate) tx: super::EventSender,
    pub(crate) index_events: Option<super::index_events::IndexEvents>,
    pub(crate) scanning: bool,
    pub(crate) active_scans: usize,
    pub(crate) cache_path: PathBuf,
    pub(crate) settings_path: PathBuf,
    pub(crate) shortcuts: crate::shortcuts::ShortcutSettings,
    pub(crate) selected_roots: Vec<PathBuf>,
    pub(crate) scan_roots: Vec<PathBuf>,
    pub(crate) onboarding_complete: bool,
    pub(crate) onboarding_scan: bool,
    pub(crate) setup_focus_requested: bool,
    pub(crate) cache_dirty: bool,
    pub(crate) building_cache: bool,
    pub(crate) last_cache: Instant,
    pub(crate) last_generation: u64,
    pub(crate) selected: Option<String>,
    pub(crate) native_drag: Option<(
        Box<dyn crate::transport::native_drag::NativeDragSource>,
        Instant,
    )>,
    pub(crate) file_focus: Option<egui::Id>,
    pub(crate) selection: super::selection::Selection,
    pub(crate) file_operations: super::file_operations::FileOperations,
    pub(crate) view_mode: ui::ResultView,
    pub(crate) kind_filter: ui::KindFilter,
    pub(crate) sort_mode: ui::SortMode,
    pub(crate) search_mode: ui::SearchMode,
    pub(crate) case_sensitive: bool,
    pub(crate) regex_mode: bool,
    pub(crate) whole_word: bool,
    pub(crate) ignore_accents: bool,
    pub(crate) scope_root: Option<String>,
    pub(crate) diagnostics_open: bool,
    pub(crate) sidebar_tab: ui::SidebarTab,
    /// A cancel request is in flight; scan starters stay disabled until the
    /// helper child dies and `ScanCancelled` arrives.
    pub(crate) cancelling: bool,
    pub(crate) scan_cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Staged record counts per mount for the running scan (progress %).
    pub(crate) staged_by_mount: std::collections::HashMap<String, u64>,
    /// Last completed per-mount totals, loaded from settings.
    pub(crate) mount_totals: std::collections::BTreeMap<String, u64>,
    pub(crate) about_open: bool,
    pub(crate) search_focus_requested: bool,
    pub(crate) tree_fraction: f32,
    pub(crate) tree_vertical_fraction: f32,
    pub(crate) treemap_path: String,
    pub(crate) tree_expanded: BTreeSet<String>,
    pub(crate) tree_model: Option<ui::Hierarchy>,
    pub(crate) map_file_key: Option<super::map_queries::MapKey>,
    pub(crate) map_files: Vec<neutra_core::FileRecord>,
    pub(crate) map_files_pending: bool,
    pub(crate) tree_building: bool,
    /// The shallow folder summary is being built; the tree waits for it.
    pub(crate) tree_summary_pending: bool,
    pub(crate) rank_pending: bool,
    pub(crate) rank_generation: Option<u64>,
    pub(crate) delta_stamp: Option<Option<(std::time::SystemTime, u64)>>,
    pub(crate) delta_checked: Instant,
    /// Directories with a fetch in flight. Guards against duplicate spawns
    /// while a slow subtree scan runs.
    pub(crate) tree_pending: BTreeSet<String>,
    pub(crate) remote_watcher_started: bool,
}

struct Restored {
    compact: Option<std::sync::Arc<CompactIndex>>,
    index: Index,
    cache_error: Option<String>,
}

impl NeutraApp {
    pub(crate) fn new(cc: &eframe::CreationContext<'_>) -> Self {
        ui::widgets::configure(&cc.egui_ctx);
        let logo = load_logo(&cc.egui_ctx);
        let cache_path = compact_cache_path();
        let restored = restore_durable(&cache_path);
        let has_durable_index = restored.compact.is_some();
        let cache_error = restored.cache_error.clone();
        let settings_path = gui_settings_path();
        let saved_settings = load_gui_settings(&settings_path);
        let first_run = saved_settings.is_none();
        let settings = saved_settings.unwrap_or_else(default_settings);
        let (tx, rx) = std::sync::mpsc::channel();
        let tx = super::EventSender::new(tx, cc.egui_ctx.clone());
        let index_events = super::index_events::IndexEvents::start(&cache_path, tx.clone()).ok();
        let mut app = Self {
            index: restored.index,
            compact: restored.compact,
            logo,
            scan_index: None,
            scan_failed: false,
            scan_cursors: Vec::new(),
            query: startup_query(),
            hits: Vec::new(),
            search_stats: SearchStats::default(),
            page_offset: 0,
            page_cursors: vec![None],
            last_query: None,
            search_roots: Vec::new(),
            search_excluded: Vec::new(),
            search_seq: 0,
            searching: false,
            search_tx: super::search_worker::spawn(tx.clone()),
            matcher: None,
            sort_reversed: settings.sort_reversed,
            lanes: BTreeMap::new(),
            rx,
            tx: tx.clone(),
            index_events,
            scanning: false,
            active_scans: 0,
            cache_path,
            settings_path,
            shortcuts: crate::shortcuts::ShortcutSettings::start(&cc.egui_ctx),
            selected_roots: settings.roots,
            scan_roots: Vec::new(),
            onboarding_complete: settings.onboarding_complete,
            onboarding_scan: false,
            setup_focus_requested: true,
            cache_dirty: false,
            building_cache: false,
            last_cache: Instant::now(),
            last_generation: 0,
            selected: None,
            native_drag: None,
            file_focus: None,
            selection: super::selection::Selection::default(),
            file_operations: super::file_operations::FileOperations::new(cc.egui_ctx.clone()),
            view_mode: settings.view,
            // The filter always resets to All on launch: a leftover Audio
            // or Images preset from last time would silently hide results.
            // It is deliberately not persisted.
            kind_filter: ui::KindFilter::All,
            sort_mode: settings.sort_mode,
            search_mode: if settings.search_mode == ui::SearchMode::NameAndPath {
                ui::SearchMode::Name
            } else {
                settings.search_mode
            },
            case_sensitive: settings.case_sensitive,
            regex_mode: settings.regex_mode,
            whole_word: settings.whole_word,
            ignore_accents: settings.ignore_accents,
            scope_root: None,
            diagnostics_open: env_flag("NEUTRASEARCH_GUI_DIAGNOSTICS"),
            sidebar_tab: ui::SidebarTab::default(),
            cancelling: false,
            scan_cancelled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            staged_by_mount: std::collections::HashMap::new(),
            mount_totals: settings.last_mount_totals,
            about_open: false,
            search_focus_requested: false,
            tree_fraction: 0.23,
            tree_vertical_fraction: 0.34,
            treemap_path: std::env::var("NEUTRASEARCH_GUI_TREEMAP_PATH")
                .unwrap_or_else(|_| "/".into()),
            tree_expanded: BTreeSet::from(["/".into()]),
            tree_model: None,
            map_file_key: None,
            map_files: Vec::new(),
            map_files_pending: false,
            tree_building: false,
            tree_summary_pending: false,
            rank_pending: false,
            rank_generation: None,
            delta_stamp: None,
            delta_checked: Instant::now(),
            tree_pending: BTreeSet::new(),
            remote_watcher_started: false,
        };
        app.finish_startup(first_run, has_durable_index, cache_error, tx);
        app
    }
}

mod bootstrap;
mod startup;
use bootstrap::{default_settings, load_logo, restore_durable, startup_query};
mod runtime;
