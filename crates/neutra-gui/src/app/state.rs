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
use std::sync::mpsc::{Receiver, Sender};
use std::time::Instant;

use eframe::egui;

use crate::ui;

pub(crate) struct NeutraApp {
    pub(crate) index: Index,
    pub(crate) compact: Option<CompactIndex>,
    pub(crate) logo: egui::TextureHandle,
    pub(crate) scan_index: Option<SpillAccumulator>,
    pub(crate) query: String,
    pub(crate) hits: Vec<SearchHit>,
    pub(crate) search_stats: SearchStats,
    /// Newest queued search id and whether its results are still pending.
    pub(crate) search_seq: u64,
    pub(crate) searching: bool,
    pub(crate) search_tx: Sender<super::search_worker::SearchJob>,
    /// Compiled text matcher for the current query, reused by result
    /// highlighting so a regex compiles once per query, not per row per frame.
    pub(crate) matcher: Option<neutra_core::QueryMatcher>,
    pub(crate) sort_reversed: bool,
    pub(crate) lanes: BTreeMap<String, LaneState>,
    pub(crate) rx: Receiver<Event>,
    pub(crate) tx: Sender<Event>,
    pub(crate) scanning: bool,
    pub(crate) active_scans: usize,
    pub(crate) cache_path: PathBuf,
    pub(crate) settings_path: PathBuf,
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
    /// Wall-clock start of the running scan, for matching the helper child
    /// by process birth time on cancel.
    pub(crate) scan_started: std::time::SystemTime,
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
     pub(crate) tree_building: bool,
     /// The shallow folder summary is being built; the tree waits for it.
     pub(crate) tree_summary_pending: bool,
     /// Directories with a fetch in flight. Guards against duplicate spawns
     /// while a slow subtree scan runs.
     pub(crate) tree_pending: BTreeSet<String>,
    pub(crate) remote_watcher_started: bool,
}

struct Restored {
    compact: Option<CompactIndex>,
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
        let mut app = Self {
            index: restored.index,
            compact: restored.compact,
            logo,
            scan_index: None,
             query: startup_query(),
            hits: Vec::new(),
            search_stats: SearchStats::default(),
            search_seq: 0,
            searching: false,
            search_tx: super::search_worker::spawn(tx.clone(), cc.egui_ctx.clone()),
            matcher: None,
            sort_reversed: settings.sort_reversed,
            lanes: BTreeMap::new(),
            rx,
            tx: tx.clone(),
            scanning: false,
            active_scans: 0,
            cache_path,
            settings_path,
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
            scan_started: std::time::SystemTime::now(),
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
             tree_building: false,
             tree_summary_pending: false,
             tree_pending: BTreeSet::new(),
            remote_watcher_started: false,
        };
        app.seed_lanes(has_durable_index, cache_error, first_run);
        // Settings files written by older versions may still record an
        // unfinished setup run; the folder question is gone, so complete
        // setup here and let a failed scan surface through the access
        // banner instead.
        if !app.onboarding_complete {
            app.onboarding_complete = true;
            app.save_settings();
        }
        app.requery();
        app.ensure_tree_summary();
         if env_flag("NEUTRASEARCH_AUTO_PROVISION_REMOTE") {
            crate::transport::spawn_network_watcher(tx);
            app.remote_watcher_started = true;
        }
         if app.index_is_empty() {
            // A missing or empty index is never a valid idle first screen. This
            // also repairs partial installs that wrote settings before their
            // first usable scan completed.
            app.selected_roots = default_system_roots();
            app.onboarding_scan = true;
            app.save_settings();
            app.begin_scan_with_elevation(cfg!(target_os = "linux"));
         } else if !env_flag("NEUTRASEARCH_NO_AUTOSCAN") {
            // Every launch re-scans the configured roots (everything by
            // default); the previous index stays searchable until the new
            // one lands. Opt out with NEUTRASEARCH_NO_AUTOSCAN=1.
            app.begin_scan_with_elevation(cfg!(target_os = "linux"));
        }
        app
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

 fn restore_durable(cache_path: &std::path::Path) -> Restored {
     if !cache_path.is_file() {
        let index = std::fs::read(legacy_cache_path())
            .ok()
            .and_then(|bytes| Index::restore(&bytes).ok())
            .unwrap_or_default();
        return Restored {
            compact: None,
            index,
            cache_error: None,
        };
    }
    match CompactIndex::open_fast(cache_path) {
        Ok(compact) => Restored {
            compact: Some(compact),
            index: Index::default(),
            cache_error: None,
        },
        Err(error) => Restored {
            compact: None,
            index: Index::default(),
            cache_error: Some(format!(
                "cannot open durable index {}: {error}",
                cache_path.display()
            )),
        },
    }
}

 fn default_settings() -> GuiSettings {
     // Setup never asks which folders to scan: everything is indexed by
     // default and locations stay adjustable from the index settings.
     GuiSettings {
         onboarding_complete: true,
         roots: default_system_roots(),
         ..GuiSettings::default()
     }
 }

 fn startup_query() -> String {
     std::env::var("NEUTRASEARCH_GUI_QUERY").unwrap_or_default()
 }

fn load_logo(ctx: &egui::Context) -> egui::TextureHandle {
    let (rgba, width, height) = embedded_logo();
    ctx.load_texture(
        "neutrasearch-logo",
        egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &rgba),
        egui::TextureOptions::LINEAR,
    )
}

impl eframe::App for NeutraApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Event pump first so the frame paints fresh state; queries and scans
        // read the same fields the UI draws.
        super::events::process_events(self);
        ui::show_app(self, ui);
    }
}
