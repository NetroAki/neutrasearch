mod terminal;
mod ui;

use eframe::egui;
use neutra_core::proto::{read_frame, write_frame, ClientMsg, HelperMsg, PROTO_VERSION};
use neutra_core::{
    CompactIndex, FileKind, FileRecord, Index, MountInfo, Query, SearchHit, SearchStats, SortKey,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

enum Event {
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
}

enum FileAction {
    Open(PathBuf),
    Reveal(PathBuf),
}

#[derive(Default, Clone)]
struct LaneState {
    label: String,
    status: String,
    records: u64,
    ms: u64,
    error: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct GuiSettings {
    onboarding_complete: bool,
    roots: Vec<PathBuf>,
}

struct NeutraApp {
    index: Index,
    compact: Option<CompactIndex>,
    logo: egui::TextureHandle,
    scan_index: Option<Index>,
    query: String,
    hits: Vec<SearchHit>,
    search_stats: SearchStats,
    lanes: BTreeMap<String, LaneState>,
    rx: Receiver<Event>,
    tx: Sender<Event>,
    scanning: bool,
    active_scans: usize,
    cache_path: PathBuf,
    settings_path: PathBuf,
    selected_roots: Vec<PathBuf>,
    scan_roots: Vec<PathBuf>,
    onboarding_complete: bool,
    onboarding_scan: bool,
    setup_focus_requested: bool,
    cache_dirty: bool,
    building_cache: bool,
    last_cache: Instant,
    last_generation: u64,
    selected: Option<String>,
    view_mode: ui::ResultView,
    kind_filter: ui::KindFilter,
    sort_mode: ui::SortMode,
    search_mode: ui::SearchMode,
    case_sensitive: bool,
    regex_mode: bool,
    scope_root: Option<String>,
    diagnostics_open: bool,
    about_open: bool,
    search_focus_requested: bool,
    tree_fraction: f32,
    tree_vertical_fraction: f32,
    treemap_path: String,
    tree_expanded: BTreeSet<String>,
    tree_model: Option<ui::Hierarchy>,
    tree_building: bool,
    remote_watcher_started: bool,
}

fn main() -> eframe::Result<()> {
    match terminal::action() {
        terminal::Action::Gui => {}
        terminal::Action::Exit(code) => std::process::exit(code),
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([760.0, 500.0])
            .with_title("Neutrasearch")
            .with_app_id("neutrasearch")
            .with_icon(app_icon()),
        renderer: eframe::Renderer::Glow,
        // Wayland interactive resize should track the pointer instead of waiting
        // for the next compositor-synchronized frame.
        vsync: false,
        ..Default::default()
    };
    eframe::run_native(
        "Neutrasearch",
        options,
        Box::new(|cc| Ok(Box::new(NeutraApp::new(cc)))),
    )
}

fn embedded_logo() -> (Vec<u8>, u32, u32) {
    let image = image::load_from_memory(include_bytes!("../assets/neutrasearch.png"))
        .expect("embedded Neutrasearch icon must decode")
        .into_rgba8();
    let (width, height) = image.dimensions();
    (image.into_raw(), width, height)
}

fn app_icon() -> egui::IconData {
    let (rgba, width, height) = embedded_logo();
    egui::IconData {
        rgba,
        width,
        height,
    }
}

impl NeutraApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        ui::configure(&cc.egui_ctx);
        let (logo_rgba, logo_width, logo_height) = embedded_logo();
        let logo = cc.egui_ctx.load_texture(
            "neutrasearch-logo",
            egui::ColorImage::from_rgba_unmultiplied(
                [logo_width as usize, logo_height as usize],
                &logo_rgba,
            ),
            egui::TextureOptions::LINEAR,
        );
        let reference_mode = env_flag("NEUTRASEARCH_GUI_REFERENCE", "NEUTRA_GUI_REFERENCE");
        let cache_path = compact_cache_path();
        let (compact, cache_error) = if reference_mode {
            (None, None)
        } else if cache_path.is_file() {
            match CompactIndex::open(&cache_path) {
                Ok(index) => (Some(index), None),
                Err(error) => (
                    None,
                    Some(format!(
                        "cannot open durable index {}: {error}",
                        cache_path.display()
                    )),
                ),
            }
        } else {
            (None, None)
        };
        let restored = if reference_mode {
            Some(ui::reference_index())
        } else if compact.is_none() {
            std::fs::read(legacy_cache_path())
                .ok()
                .and_then(|b| Index::restore(&b).ok())
        } else {
            None
        };
        let has_durable_index = compact.is_some() || restored.is_some();
        let index = restored.unwrap_or_default();
        let settings_path = gui_settings_path();
        let saved_settings = (!reference_mode)
            .then(|| load_gui_settings(&settings_path))
            .flatten();
        let first_run = !reference_mode && saved_settings.is_none();
        let settings = if reference_mode {
            GuiSettings {
                onboarding_complete: true,
                roots: vec![PathBuf::from("/")],
            }
        } else {
            saved_settings.unwrap_or_else(|| GuiSettings {
                onboarding_complete: false,
                roots: default_system_roots(),
            })
        };
        let (tx, rx) = mpsc::channel();
        let mut app = Self {
            index,
            compact,
            logo,
            scan_index: None,
            query: std::env::var("NEUTRASEARCH_GUI_QUERY").unwrap_or_else(|_| {
                if reference_mode {
                    "invoice".into()
                } else {
                    String::new()
                }
            }),
            hits: Vec::new(),
            search_stats: SearchStats::default(),
            lanes: BTreeMap::new(),
            rx,
            tx,
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
            view_mode: ui::initial_view(),
            kind_filter: ui::KindFilter::All,
            sort_mode: ui::SortMode::Modified,
            search_mode: ui::SearchMode::NameAndPath,
            case_sensitive: false,
            regex_mode: env_flag("NEUTRASEARCH_GUI_REGEX", "NEUTRA_GUI_REGEX"),
            scope_root: None,
            diagnostics_open: env_flag("NEUTRASEARCH_GUI_DIAGNOSTICS", "NEUTRA_GUI_DIAGNOSTICS"),
            about_open: false,
            search_focus_requested: false,
            tree_fraction: 0.23,
            tree_vertical_fraction: 0.34,
            treemap_path: std::env::var("NEUTRASEARCH_GUI_TREEMAP_PATH")
                .unwrap_or_else(|_| "/".into()),
            tree_expanded: BTreeSet::from(["/".into()]),
            tree_model: None,
            tree_building: false,
            remote_watcher_started: false,
        };
        if has_durable_index {
            app.lanes.insert(
                "cache".into(),
                LaneState {
                    label: "DURABLE INDEX".into(),
                    status: format!("restored {} entries", app.index_len()),
                    records: app.index_len(),
                    ..Default::default()
                },
            );
        }
        if let Some(error) = cache_error {
            app.lanes.insert(
                "cache-error".into(),
                LaneState {
                    label: "INDEX ERROR".into(),
                    status: error,
                    error: true,
                    ..Default::default()
                },
            );
        } else if !has_durable_index {
            app.lanes.insert(
                "welcome".into(),
                LaneState {
                    label: "READY".into(),
                    status: if first_run {
                        "Scanning all local system drives automatically".into()
                    } else {
                        "Choose Scan to build the local index".into()
                    },
                    ..Default::default()
                },
            );
        }
        app.requery();
        if !reference_mode
            && env_flag(
                "NEUTRASEARCH_AUTO_PROVISION_REMOTE",
                "NEUTRA_AUTO_PROVISION_REMOTE",
            )
        {
            spawn_network_watcher(app.tx.clone());
            app.remote_watcher_started = true;
        }
        if first_run && !app.selected_roots.is_empty() {
            // A fresh install indexes the complete local machine immediately.
            // Setup remains incomplete until a native lane returns usable data.
            app.onboarding_scan = true;
            app.save_settings();
            app.begin_scan_with_elevation(cfg!(target_os = "linux"));
        } else if !reference_mode
            && (env_flag("NEUTRASEARCH_AUTOSCAN", "NEUTRA_AUTOSCAN")
                || env_flag("NEUTRASEARCH_FORCE_RESCAN", "NEUTRA_FORCE_RESCAN"))
        {
            app.begin_scan();
        }
        app
    }

    fn begin_scan(&mut self) {
        self.begin_scan_with_elevation(false);
    }

    fn begin_scan_with_elevation(&mut self, elevated: bool) {
        if self.scanning || self.building_cache {
            return;
        }
        if self.selected_roots.is_empty() {
            self.compact = None;
            self.index = Index::new();
            self.tree_model = None;
            self.tree_building = false;
            self.cache_dirty = true;
            self.last_cache = Instant::now() - Duration::from_secs(3);
            self.lanes.clear();
            self.lanes.insert(
                "locations".into(),
                LaneState {
                    label: "SEARCH LOCATIONS".into(),
                    status: "no folders selected".into(),
                    ..Default::default()
                },
            );
            self.requery();
            return;
        }
        // Build into a staging index. The last complete index remains searchable
        // until at least one requested native lane completes successfully.
        self.scan_index = Some(Index::new());
        self.scan_roots = self.selected_roots.clone();
        self.scanning = true;
        self.active_scans = 0;
        self.cache_dirty = false;
        self.lanes.clear();
        let mounts = selected_scan_mounts(&self.selected_roots);
        if mounts.is_empty() {
            self.scanning = false;
            self.scan_index = None;
            self.scan_roots.clear();
            self.onboarding_scan = false;
            self.lanes.insert(
                "locations".into(),
                LaneState {
                    label: "SEARCH LOCATIONS".into(),
                    status: "selected folders are not on a supported local native filesystem"
                        .into(),
                    error: true,
                    ..Default::default()
                },
            );
            return;
        }
        spawn_local_helper(self.tx.clone(), elevated, mounts, self.scan_roots.clone());
    }

    fn complete_onboarding_and_scan(&mut self) {
        if self.selected_roots.is_empty() {
            return;
        }
        // Persist the chosen roots, but do not dismiss setup until at least one
        // requested native lane has produced a usable index.
        self.onboarding_complete = false;
        self.onboarding_scan = true;
        self.save_settings();
        self.begin_scan_with_elevation(cfg!(target_os = "linux"));
    }

    fn add_root(&mut self, root: PathBuf) {
        let root = normalize_selected_root(std::fs::canonicalize(&root).unwrap_or(root));
        if root.is_absolute()
            && !self
                .selected_roots
                .iter()
                .any(|existing| same_root(existing, &root))
        {
            self.selected_roots.push(root);
            self.selected_roots.sort();
            self.scope_root = None;
            self.setup_focus_requested = true;
            if self.onboarding_complete {
                self.save_settings();
                self.requery();
                self.begin_scan_with_elevation(cfg!(target_os = "linux"));
            }
        }
    }

    fn remove_root(&mut self, index: usize) {
        if index < self.selected_roots.len() {
            self.selected_roots.remove(index);
            self.scope_root = None;
            self.save_settings();
            self.requery();
            if self.onboarding_complete {
                self.begin_scan_with_elevation(cfg!(target_os = "linux"));
            }
        }
    }

    fn save_settings(&mut self) {
        let settings = GuiSettings {
            onboarding_complete: self.onboarding_complete,
            roots: self.selected_roots.clone(),
        };
        if let Err(error) = save_gui_settings(&self.settings_path, &settings) {
            self.lanes.insert(
                "settings".into(),
                LaneState {
                    label: "SETTINGS".into(),
                    status: error,
                    error: true,
                    ..Default::default()
                },
            );
        }
    }
    fn process_events(&mut self) -> bool {
        const MAX_EVENTS_PER_FRAME: usize = 64;
        let mut processed = 0;
        while processed < MAX_EVENTS_PER_FRAME {
            let Ok(ev) = self.rx.try_recv() else { break };
            processed += 1;
            match ev {
                Event::Fatal(e) => {
                    self.scanning = false;
                    self.active_scans = 0;
                    self.scan_index = None;
                    self.scan_roots.clear();
                    self.onboarding_scan = false;
                    self.setup_focus_requested = true;
                    self.lanes.insert(
                        "helper".into(),
                        LaneState {
                            label: "HELPER".into(),
                            status: e,
                            records: 0,
                            ms: 0,
                            error: true,
                        },
                    );
                }
                Event::Remote { key, status, error } => {
                    self.lanes.insert(
                        format!("remote:{key}"),
                        LaneState {
                            label: format!("REMOTE/{key}"),
                            status,
                            records: 0,
                            ms: 0,
                            error,
                        },
                    );
                }
                Event::CompactReady(index) => {
                    self.last_generation = index.generation();
                    self.compact = Some(index);
                    // The resident copy stayed searchable throughout the build;
                    // reclaim it only after the replacement mmap is verified.
                    self.index = Index::new();
                    self.building_cache = false;
                    self.cache_dirty = false;
                    self.last_cache = Instant::now();
                    self.lanes.insert(
                        "cache".into(),
                        LaneState {
                            label: "COMPACT MMAP".into(),
                            status: "published; idle pages are reclaimable".into(),
                            records: self.index_len(),
                            ..Default::default()
                        },
                    );
                    self.requery();
                }
                Event::CompactFailed(error) => {
                    // Keep serving the complete resident index when cache
                    // publication fails.
                    self.building_cache = false;
                    self.cache_dirty = false;
                    self.lanes.insert(
                        "cache".into(),
                        LaneState {
                            label: "INDEX BUILD".into(),
                            status: error,
                            error: true,
                            ..Default::default()
                        },
                    );
                    self.requery();
                }
                Event::TreeReady { generation, model } => {
                    self.tree_building = false;
                    if generation == self.data_generation() {
                        self.tree_model = Some(model);
                    }
                }
                Event::TreeFailed(error) => {
                    self.tree_building = false;
                    self.lanes.insert(
                        "tree".into(),
                        LaneState {
                            label: "DISK MAP".into(),
                            status: error,
                            error: true,
                            ..Default::default()
                        },
                    );
                }
                Event::Message(msg) => match msg {
                    HelperMsg::Hello { os, arch, .. } => {
                        self.lanes.insert(
                            "host".into(),
                            LaneState {
                                label: format!("{os}/{arch}"),
                                status: "native helper online".into(),
                                ..Default::default()
                            },
                        );
                    }
                    HelperMsg::ScanBegin { mount } => {
                        self.active_scans += 1;
                        let k = mount.mountpoint.display().to_string();
                        self.lanes.insert(
                            k.clone(),
                            LaneState {
                                label: mount.fs.label().to_uppercase(),
                                status: format!("indexing {k}"),
                                ..Default::default()
                            },
                        );
                    }
                    HelperMsg::Records(records) => {
                        let roots = &self.scan_roots;
                        if let Some(staging) = &mut self.scan_index {
                            staging.extend(
                                records
                                    .into_iter()
                                    .filter(|record| record_in_roots(record.path.as_ref(), roots)),
                            );
                        }
                    }
                    HelperMsg::ScanDone { mount, stats } => {
                        self.active_scans = self.active_scans.saturating_sub(1);
                        let k = mount.mountpoint.display().to_string();
                        self.lanes.insert(
                            k,
                            LaneState {
                                label: mount.fs.label().to_uppercase(),
                                status: stats.detail,
                                records: stats.records,
                                ms: stats.wall_ms,
                                error: false,
                            },
                        );
                    }
                    HelperMsg::ScanError { mount, error } => {
                        self.active_scans = self.active_scans.saturating_sub(1);
                        let k = mount.mountpoint.display().to_string();
                        self.lanes.insert(
                            k,
                            LaneState {
                                label: mount.fs.label().to_uppercase(),
                                status: error,
                                records: 0,
                                ms: 0,
                                error: true,
                            },
                        );
                    }
                    HelperMsg::ScanComplete { mounts, errors } => {
                        self.scanning = false;
                        self.active_scans = 0;
                        let staging = self.scan_index.take();
                        self.scan_roots.clear();
                        if mounts == 0 {
                            self.onboarding_scan = false;
                            self.setup_focus_requested = true;
                            self.lanes.insert(
                                "scan".into(),
                                LaneState {
                                    label: "NATIVE SCAN".into(),
                                    status: format!(
                                        "no supported native filesystems were discovered on {}",
                                        std::env::consts::OS
                                    ),
                                    error: true,
                                    ..Default::default()
                                },
                            );
                        } else if scan_has_reachable_lane(mounts, errors) {
                            let staging = staging.unwrap_or_default();
                            if self.onboarding_scan || !self.onboarding_complete {
                                self.onboarding_complete = true;
                                self.onboarding_scan = false;
                                self.save_settings();
                            }
                            self.compact = None;
                            self.index = staging;
                            self.tree_model = None;
                            self.tree_building = false;
                            self.cache_dirty = true;
                            self.last_cache = Instant::now() - Duration::from_secs(3);
                            if errors > 0 {
                                self.lanes.insert(
                                    "scan".into(),
                                    LaneState {
                                        label: "PARTIAL INDEX".into(),
                                        status: format!(
                                            "indexed reachable locations; skipped {errors} unavailable native lane(s)"
                                        ),
                                        error: false,
                                        ..Default::default()
                                    },
                                );
                            }
                            self.requery();
                        } else if errors > 0 {
                            self.onboarding_scan = false;
                            self.setup_focus_requested = true;
                            self.lanes.insert(
                                "scan".into(),
                                LaneState {
                                    label: "NO REACHABLE LOCATIONS".into(),
                                    status: format!(
                                        "all {errors} unavailable native lane(s) were skipped; keeping the last complete index"
                                    ),
                                    error: true,
                                    ..Default::default()
                                },
                            );
                        }
                    }
                    HelperMsg::Error(e) => {
                        self.scanning = false;
                        self.active_scans = 0;
                        self.scan_index = None;
                        self.scan_roots.clear();
                        self.onboarding_scan = false;
                        self.setup_focus_requested = true;
                        self.lanes.insert(
                            "protocol".into(),
                            LaneState {
                                label: "PROTOCOL".into(),
                                status: e,
                                records: 0,
                                ms: 0,
                                error: true,
                            },
                        );
                    }
                    HelperMsg::SearchResult { .. } => {}
                    HelperMsg::DeltaApplied {
                        changes,
                        wal_bytes,
                        needs_compaction,
                    } => {
                        self.lanes.insert(
                            "delta".into(),
                            LaneState {
                                label: "LIVE DELTA".into(),
                                status: format!(
                                    "{changes} changes · {wal_bytes} bytes{}",
                                    if needs_compaction {
                                        " · compaction due"
                                    } else {
                                        ""
                                    }
                                ),
                                ..Default::default()
                            },
                        );
                    }
                },
            }
        }
        let generation = self.data_generation();
        if generation != self.last_generation {
            self.last_generation = generation;
            self.tree_model = None;
            self.requery();
        }
        if self.cache_dirty
            && !self.building_cache
            && self.active_scans == 0
            && self.last_cache.elapsed() > Duration::from_secs(2)
        {
            let records = self.index.records().to_vec();
            let generation = self.data_generation();
            let path = self.cache_path.clone();
            let tx = self.tx.clone();
            self.building_cache = true;
            self.tree_building = true;
            self.lanes.insert(
                "cache".into(),
                LaneState {
                    label: "COMPACT INDEX".into(),
                    status: "building compressed search blocks".into(),
                    records: records.len() as u64,
                    ..Default::default()
                },
            );
            std::thread::spawn(move || {
                let model = ui::Hierarchy::from_records(&records);
                let _ = tx.send(Event::TreeReady { generation, model });
                match CompactIndex::build(&records, &path).and_then(|_| CompactIndex::open(&path)) {
                    Ok(compact) => {
                        let _ = tx.send(Event::CompactReady(compact));
                    }
                    Err(error) => {
                        let _ = tx.send(Event::CompactFailed(error.to_string()));
                    }
                }
            });
        }
        processed == MAX_EVENTS_PER_FRAME
    }
    fn index_len(&self) -> u64 {
        self.compact
            .as_ref()
            .map_or(self.index.len() as u64, CompactIndex::len)
    }
    fn index_is_empty(&self) -> bool {
        self.index_len() == 0
    }
    fn scan_len(&self) -> u64 {
        self.scan_index
            .as_ref()
            .map_or(0, |index| index.len() as u64)
    }
    fn data_generation(&self) -> u64 {
        self.compact
            .as_ref()
            .map_or_else(|| self.index.generation(), CompactIndex::generation)
    }
    fn request_tree_model(&mut self) {
        if self.tree_building || self.tree_model.is_some() || self.index_is_empty() {
            return;
        }
        self.tree_building = true;
        let generation = self.data_generation();
        let tx = self.tx.clone();
        let compact_path = self.compact.as_ref().map(|_| self.cache_path.clone());
        let memory_records = compact_path
            .is_none()
            .then(|| self.index.records().to_vec());
        std::thread::spawn(move || {
            let records: Result<Vec<FileRecord>, String> = if let Some(path) = compact_path {
                CompactIndex::open(&path)
                    .and_then(|index| index.records())
                    .map_err(|error| format!("cannot prepare disk hierarchy: {error}"))
            } else {
                Ok(memory_records.unwrap_or_default())
            };
            match records {
                Ok(records) => {
                    let model = ui::Hierarchy::from_records(&records);
                    let _ = tx.send(Event::TreeReady { generation, model });
                }
                Err(error) => {
                    let _ = tx.send(Event::TreeFailed(error));
                }
            }
        });
    }
    fn requery(&mut self) {
        if self.selected_roots.is_empty() && self.onboarding_complete {
            self.hits.clear();
            self.search_stats = SearchStats::default();
            return;
        }
        if let Some(current_generation) = self.compact.as_ref().map(CompactIndex::generation) {
            match CompactIndex::generation_on_disk(&self.cache_path) {
                Ok(on_disk) if on_disk == current_generation => {}
                Ok(_) => match CompactIndex::open(&self.cache_path) {
                    Ok(index) => self.compact = Some(index),
                    Err(error) => {
                        self.hits.clear();
                        self.lanes.insert(
                            "index".into(),
                            LaneState {
                                label: "Durable index".into(),
                                status: format!("replacement rejected: {error}"),
                                error: true,
                                ..LaneState::default()
                            },
                        );
                        return;
                    }
                },
                Err(error) => {
                    self.hits.clear();
                    self.lanes.insert(
                        "index".into(),
                        LaneState {
                            label: "Durable index".into(),
                            status: format!("unavailable: {error}"),
                            error: true,
                            ..LaneState::default()
                        },
                    );
                    return;
                }
            }
        }
        let mut q = Query::parse(if self.regex_mode { "" } else { &self.query });
        q.limit = 1_000;
        q.sort = match self.sort_mode {
            ui::SortMode::Modified => SortKey::MtimeDesc,
            ui::SortMode::Name => SortKey::NameAsc,
            ui::SortMode::Size => SortKey::SizeDesc,
            ui::SortMode::Path => SortKey::PathAsc,
        };
        q.kinds = match self.kind_filter {
            ui::KindFilter::All => Vec::new(),
            ui::KindFilter::Files => vec![FileKind::File, FileKind::Symlink],
            ui::KindFilter::Folders => vec![FileKind::Dir],
        };
        let scoped_root = self
            .scope_root
            .as_deref()
            .filter(|scope| scope_within_selected_roots(scope, &self.selected_roots));
        if let Some(root) = scoped_root {
            q.scope_roots.push(root.to_owned());
        } else {
            q.scope_roots.extend(
                self.selected_roots
                    .iter()
                    .map(|root| root.to_string_lossy().into_owned()),
            );
        }
        q.scope_case_sensitive = cfg!(not(any(target_os = "windows", target_os = "macos")));
        let result = if let Some(index) = &self.compact {
            index.search(&q).ok()
        } else {
            Some(self.index.search(&q))
        };
        if let Some((hits, stats)) = result {
            self.hits = hits;
            self.search_stats = stats;
        }
    }
}

impl eframe::App for NeutraApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui::show_app(self, ui);
    }
}

#[cfg(target_os = "windows")]
fn request_elevated_restart() -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;

    #[allow(non_snake_case)]
    #[link(name = "shell32")]
    extern "system" {
        fn ShellExecuteW(
            window: *mut std::ffi::c_void,
            operation: *const u16,
            file: *const u16,
            parameters: *const u16,
            directory: *const u16,
            show: i32,
        ) -> *mut std::ffi::c_void;
    }

    let executable = std::env::current_exe()
        .map_err(|error| format!("cannot locate Neutrasearch executable: {error}"))?;
    let operation = std::ffi::OsStr::new("runas")
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let executable = executable
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            executable.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
        )
    } as isize;
    if result <= 32 {
        Err(format!(
            "Windows elevation request failed with code {result}"
        ))
    } else {
        Ok(())
    }
}

#[cfg(not(target_os = "windows"))]
fn request_elevated_restart() -> Result<(), String> {
    Err("elevated restart is only available on Windows".into())
}

fn launch_file_action(action: FileAction) -> std::io::Result<()> {
    let mut command = match action {
        FileAction::Open(path) => {
            #[cfg(target_os = "windows")]
            {
                let mut command = Command::new("explorer.exe");
                command.arg(path);
                command
            }
            #[cfg(target_os = "macos")]
            {
                let mut command = Command::new("open");
                command.arg(path);
                command
            }
            #[cfg(not(any(target_os = "windows", target_os = "macos")))]
            {
                let mut command = Command::new("xdg-open");
                command.arg(path);
                command
            }
        }
        FileAction::Reveal(path) => {
            #[cfg(target_os = "windows")]
            {
                let mut command = Command::new("explorer.exe");
                command.arg(format!("/select,{}", path.display()));
                command
            }
            #[cfg(target_os = "macos")]
            {
                let mut command = Command::new("open");
                command.arg("-R").arg(path);
                command
            }
            #[cfg(not(any(target_os = "windows", target_os = "macos")))]
            {
                let mut command = Command::new("xdg-open");
                command.arg(path.parent().unwrap_or(&path));
                command
            }
        }
    };
    command.spawn()?;
    Ok(())
}

fn select_helper(
    configured: Option<PathBuf>,
    current_exe: Option<PathBuf>,
    elevated: bool,
) -> Result<PathBuf, String> {
    if elevated && configured.is_some() {
        return Err(
            "refusing to elevate a helper selected through NEUTRASEARCH_HELPER; install a trusted system helper"
                .into(),
        );
    }
    let sibling = current_exe.map(|path| {
        path.with_file_name(if cfg!(windows) {
            "neutrasearch-helper.exe"
        } else {
            "neutrasearch-helper"
        })
    });
    if elevated {
        #[cfg(unix)]
        let candidates = [
            PathBuf::from("/usr/local/lib/neutrasearch/neutrasearch-helper"),
            PathBuf::from("/usr/lib/neutrasearch/neutrasearch-helper"),
            PathBuf::from("/usr/local/bin/neutrasearch-helper"),
        ];
        #[cfg(not(unix))]
        let candidates = sibling.into_iter().collect::<Vec<_>>();
        for helper in candidates {
            if let Ok(helper) = validate_elevated_helper(&helper) {
                return Ok(helper);
            }
        }
        return Err(
            "no trusted administrator helper is installed; reinstall Neutrasearch system-wide"
                .into(),
        );
    }
    Ok(configured
        .or(sibling)
        .unwrap_or_else(|| PathBuf::from("neutrasearch-helper")))
}

#[cfg(unix)]
fn validate_elevated_helper(path: &std::path::Path) -> Result<PathBuf, String> {
    use std::os::unix::fs::MetadataExt;
    let path = std::fs::canonicalize(path).map_err(|error| {
        format!(
            "cannot resolve installed helper {}: {error}",
            path.display()
        )
    })?;
    let allowed = [
        std::path::Path::new("/usr/local/lib/neutrasearch"),
        std::path::Path::new("/usr/lib/neutrasearch"),
        std::path::Path::new("/usr/local/bin"),
    ];
    if !allowed.iter().any(|directory| path.starts_with(directory)) {
        return Err(format!(
            "refusing helper outside trusted system locations: {}",
            path.display()
        ));
    }
    let metadata = std::fs::metadata(&path).map_err(|error| {
        format!(
            "cannot inspect installed helper {}: {error}",
            path.display()
        )
    })?;
    if !metadata.is_file() || metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
        return Err(format!(
            "refusing to elevate untrusted helper {}; it must be a root-owned regular file not writable by group/others",
            path.display()
        ));
    }
    let mut ancestor = path.parent();
    while let Some(directory) = ancestor {
        let metadata = std::fs::metadata(directory).map_err(|error| {
            format!(
                "cannot inspect helper directory {}: {error}",
                directory.display()
            )
        })?;
        if !metadata.is_dir() || metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
            return Err(format!(
                "refusing helper beneath untrusted directory {}",
                directory.display()
            ));
        }
        ancestor = directory.parent();
    }
    Ok(path)
}

#[cfg(not(unix))]
fn validate_elevated_helper(path: &std::path::Path) -> Result<PathBuf, String> {
    std::fs::canonicalize(path).map_err(|error| {
        format!(
            "cannot resolve installed helper {}: {error}",
            path.display()
        )
    })
}

fn spawn_local_helper(
    tx: Sender<Event>,
    elevated_requested: bool,
    mounts: Vec<MountInfo>,
    roots: Vec<PathBuf>,
) {
    std::thread::spawn(move || {
        #[cfg(target_os = "windows")]
        match scan_via_windows_service(tx.clone(), mounts.clone(), roots.clone()) {
            Ok(true) => return,
            Ok(false) => {
                // Portable archives do not install the service. Preserve their
                // sibling-helper path (and the existing explicit elevation UX).
            }
            Err(error) => {
                let _ = tx.send(Event::Fatal(error));
                return;
            }
        }

        let configured = std::env::var_os("NEUTRASEARCH_HELPER")
            .or_else(|| std::env::var_os("NEUTRA_HELPER"))
            .map(PathBuf::from);
        let elevated = cfg!(target_os = "linux")
            && (elevated_requested
                || std::env::var_os("NEUTRASEARCH_PKEXEC").is_some()
                || std::env::var_os("NEUTRA_PKEXEC").is_some());
        let helper = match select_helper(configured, std::env::current_exe().ok(), elevated) {
            Ok(helper) => helper,
            Err(error) => {
                let _ = tx.send(Event::Fatal(error));
                return;
            }
        };
        let mut cmd = if elevated {
            let mut command = Command::new("pkexec");
            command.arg(helper);
            command
        } else {
            Command::new(helper)
        };
        let child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match child {
            Ok(child) => child,
            Err(error) => {
                let _ = tx.send(Event::Fatal(format!(
                    "cannot start the native scanner: {error}"
                )));
                return;
            }
        };
        let stderr_text = Arc::new(Mutex::new(String::new()));
        let stderr_reader = child.stderr.take().map(|stderr| {
            let captured = Arc::clone(&stderr_text);
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    eprintln!("neutrasearch-helper: {line}");
                    if let Ok(mut text) = captured.lock() {
                        if text.len() < 8_192 {
                            if !text.is_empty() {
                                text.push('\n');
                            }
                            text.push_str(&line);
                            text.truncate(8_192);
                        }
                    }
                }
            })
        });
        let Some(stdin) = child.stdin.take() else {
            let _ = child.kill();
            let _ = child.wait();
            if let Some(reader) = stderr_reader {
                let _ = reader.join();
            }
            let _ = tx.send(Event::Fatal(helper_start_failure(
                "native scanner input pipe is unavailable",
                &stderr_text,
            )));
            return;
        };
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            if let Some(reader) = stderr_reader {
                let _ = reader.join();
            }
            let _ = tx.send(Event::Fatal(helper_start_failure(
                "native scanner output pipe is unavailable",
                &stderr_text,
            )));
            return;
        };
        let (mut input, mut output) = (BufWriter::new(stdin), BufReader::new(stdout));
        if let Err(error) = write_frame(
            &mut input,
            &ClientMsg::Hello {
                proto: PROTO_VERSION,
            },
        ) {
            let _ = child.wait();
            if let Some(reader) = stderr_reader {
                let _ = reader.join();
            }
            let _ = tx.send(Event::Fatal(helper_start_failure(
                &format!("cannot contact the native scanner: {error}"),
                &stderr_text,
            )));
            return;
        }
        match read_frame::<_, HelperMsg>(&mut output) {
            Ok(Some(message)) => {
                let _ = tx.send(Event::Message(message));
            }
            other => {
                let _ = child.wait();
                if let Some(reader) = stderr_reader {
                    let _ = reader.join();
                }
                let _ = tx.send(Event::Fatal(helper_start_failure(
                    &format!("native scanner handshake failed: {other:?}"),
                    &stderr_text,
                )));
                return;
            }
        }
        if let Err(error) = write_frame(&mut input, &ClientMsg::Scan { mounts, roots }) {
            drop(input);
            let _ = child.wait();
            if let Some(reader) = stderr_reader {
                let _ = reader.join();
            }
            let _ = tx.send(Event::Fatal(helper_start_failure(
                &format!("cannot send locations to the native scanner: {error}"),
                &stderr_text,
            )));
            return;
        }
        drop(input);
        let mut completed = false;
        loop {
            match read_frame::<_, HelperMsg>(&mut output) {
                Ok(Some(message)) => {
                    completed |= matches!(message, HelperMsg::ScanComplete { .. });
                    let _ = tx.send(Event::Message(message));
                }
                Ok(None) => break,
                Err(error) => {
                    let _ = tx.send(Event::Fatal(format!("native scanner protocol: {error}")));
                    break;
                }
            }
        }
        let status = child.wait().ok();
        if let Some(reader) = stderr_reader {
            let _ = reader.join();
        }
        if !completed {
            let status_detail = status
                .map(|status| format!(" ({status})"))
                .unwrap_or_default();
            let _ = tx.send(Event::Fatal(helper_start_failure(
                &format!("the native scanner stopped before completing{status_detail}"),
                &stderr_text,
            )));
        }
    });
}

#[cfg(target_os = "windows")]
fn scan_via_windows_service(
    tx: Sender<Event>,
    mounts: Vec<MountInfo>,
    roots: Vec<PathBuf>,
) -> Result<bool, String> {
    const PIPE: &str = r"\\.\pipe\Neutrasearch.Helper.v1";
    let deadline = Instant::now() + Duration::from_secs(5);
    let pipe = loop {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(PIPE)
        {
            Ok(pipe) => break pipe,
            Err(error)
                if Instant::now() < deadline && matches!(error.raw_os_error(), Some(2 | 231)) =>
            {
                std::thread::sleep(Duration::from_millis(125));
            }
            Err(error) if error.raw_os_error() == Some(2) => {
                if windows_scanner_service_installed() {
                    return Err(
                        "the installed scanner service is unavailable; repair or restart the NeutrasearchHelper service"
                            .into(),
                    );
                }
                return Ok(false);
            }
            Err(error) if error.raw_os_error() == Some(231) => {
                return Err(
                    "the installed scanner service is busy with another scan; wait a moment and try again"
                        .into(),
                );
            }
            Err(error) => {
                return Err(format!(
                    "cannot connect to the installed scanner service: {error}; repair the Neutrasearch installation"
                ));
            }
        }
    };
    verify_windows_service_server(&pipe)?;
    let writer = pipe
        .try_clone()
        .map_err(|error| format!("cannot clone the scanner service pipe: {error}"))?;
    let mut input = BufWriter::new(writer);
    let mut output = BufReader::new(pipe);
    write_frame(
        &mut input,
        &ClientMsg::Hello {
            proto: PROTO_VERSION,
        },
    )
    .map_err(|error| format!("cannot contact the installed scanner service: {error}"))?;
    let hello: Option<HelperMsg> = read_frame(&mut output)
        .map_err(|error| format!("scanner service handshake failed: {error}"))?;
    let hello = match hello {
        Some(message @ HelperMsg::Hello { proto, .. }) if proto == PROTO_VERSION => message,
        Some(HelperMsg::Hello { proto, .. }) => {
            return Err(format!(
                "scanner service protocol mismatch: GUI={PROTO_VERSION}, service={proto}; repair the installation"
            ));
        }
        Some(message) => {
            return Err(format!(
                "scanner service returned an invalid handshake: {message:?}"
            ));
        }
        None => return Err("the installed scanner service closed during handshake".into()),
    };
    let _ = tx.send(Event::Message(hello));
    write_frame(&mut input, &ClientMsg::Scan { mounts, roots })
        .map_err(|error| format!("cannot send locations to the scanner service: {error}"))?;

    let mut completed = false;
    loop {
        match read_frame::<_, HelperMsg>(&mut output) {
            Ok(Some(message)) => {
                completed |= matches!(message, HelperMsg::ScanComplete { .. });
                let _ = tx.send(Event::Message(message));
                if completed {
                    break;
                }
            }
            Ok(None) => break,
            Err(error) => return Err(format!("scanner service protocol failed: {error}")),
        }
    }
    if !completed {
        return Err("the installed scanner service stopped before completing".into());
    }
    // End this per-client service session explicitly; the service remains
    // running and accepts the next ordinary-user scan without another UAC.
    let _ = write_frame(&mut input, &ClientMsg::Shutdown);
    Ok(true)
}

#[cfg(target_os = "windows")]
fn windows_scanner_service_installed() -> bool {
    let sc = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("System32/sc.exe");
    Command::new(sc)
        .args(["query", "NeutrasearchHelper"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(target_os = "windows")]
fn verify_windows_service_server(pipe: &std::fs::File) -> Result<(), String> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    let handle = pipe.as_raw_handle().cast();
    let mut pid = 0u32;
    if unsafe { GetNamedPipeServerProcessId(handle, &mut pid) } == 0 || pid == 0 {
        return Err(format!(
            "cannot authenticate the scanner service process: {}",
            std::io::Error::last_os_error()
        ));
    }
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return Err(format!(
            "cannot inspect the scanner service process: {}",
            std::io::Error::last_os_error()
        ));
    }
    let mut path = vec![0u16; 32_768];
    let mut length = path.len() as u32;
    let queried = unsafe { QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut length) };
    let query_error = (queried == 0).then(std::io::Error::last_os_error);
    unsafe {
        CloseHandle(process);
    }
    if let Some(error) = query_error {
        return Err(format!(
            "cannot read the scanner service executable path: {error}"
        ));
    }
    path.truncate(length as usize);
    let actual = PathBuf::from(String::from_utf16_lossy(&path));
    let expected = std::env::current_exe()
        .map_err(|error| format!("cannot locate the installed GUI: {error}"))?
        .with_file_name("neutrasearch-helper.exe");
    if !windows_paths_equal(&actual, &expected) {
        return Err(format!(
            "refusing an untrusted scanner pipe server at {}; repair the Neutrasearch installation",
            actual.display()
        ));
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn windows_paths_equal(left: &std::path::Path, right: &std::path::Path) -> bool {
    left.to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('/', "\\")
        .eq_ignore_ascii_case(
            &right
                .to_string_lossy()
                .trim_start_matches(r"\\?\")
                .replace('/', "\\"),
        )
}

fn helper_start_failure(summary: &str, stderr: &Arc<Mutex<String>>) -> String {
    let detail = stderr
        .lock()
        .map(|text| text.trim().to_owned())
        .unwrap_or_default();
    if detail.contains("textual authentication agent")
        || detail.contains("current controlling terminal")
    {
        return "administrator approval could not open in this desktop session; launch Neutrasearch from the desktop and try again".into();
    }
    if detail.is_empty() {
        summary.to_owned()
    } else {
        format!("{summary}: {detail}")
    }
}

fn spawn_network_watcher(tx: Sender<Event>) {
    std::thread::spawn(move || {
        let provisioner = neutra_remote::Provisioner::from_env();
        let mut ready = std::collections::HashSet::<String>::new();
        let mut last_attempt = BTreeMap::<String, Instant>::new();
        let mut announced_waiting = std::collections::HashSet::<String>::new();
        loop {
            for (host, key) in discover_network_hosts() {
                if ready.contains(&key)
                    || last_attempt
                        .get(&key)
                        .is_some_and(|attempt| attempt.elapsed() < Duration::from_secs(30))
                {
                    continue;
                }
                last_attempt.insert(key.clone(), Instant::now());
                match provisioner.ensure_installed(&host) {
                    Ok(platform) => {
                        ready.insert(key.clone());
                        announced_waiting.remove(&key);
                        let _ = tx.send(Event::Remote {
                            key,
                            status: format!(
                                "helper build {} ready ({:?}/{})",
                                neutra_core::proto::HELPER_BUILD,
                                platform.os,
                                platform.arch
                            ),
                            error: false,
                        });
                    }
                    Err(error) if remote_failure_is_offline(&error) => {
                        if announced_waiting.insert(key.clone()) {
                            let _ = tx.send(Event::Remote {
                                key,
                                status: "offline; waiting to retry when the server is available"
                                    .into(),
                                error: false,
                            });
                        }
                    }
                    Err(error) => {
                        ready.insert(key.clone());
                        let _ = tx.send(Event::Remote {
                            key,
                            status: format!("network helper needs attention: {error:#}"),
                            error: true,
                        });
                    }
                }
            }
            std::thread::sleep(Duration::from_secs(3));
        }
    });
}
fn scan_has_reachable_lane(mounts: u32, errors: u32) -> bool {
    mounts > 0 && errors < mounts
}

fn remote_failure_is_offline(error: &anyhow::Error) -> bool {
    let message = format!("{error:#}").to_ascii_lowercase();
    [
        "connection timed out",
        "connection refused",
        "no route to host",
        "network is unreachable",
        "could not resolve hostname",
        "name or service not known",
        "operation timed out",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

fn discover_network_hosts() -> Vec<(String, String)> {
    #[cfg(target_os = "linux")]
    {
        return neutra_core::mounts::system_mounts()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|m| {
                m.network_host()
                    .map(|h| (h, format!("{}:{}", m.device, m.mountpoint.display())))
            })
            .collect();
    }
    #[cfg(target_os = "macos")]
    {
        let out = Command::new("/sbin/mount").output().ok();
        return out
            .into_iter()
            .flat_map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .filter_map(|l| {
                let (spec, rest) = l.split_once(" on ")?;
                if !(rest.contains("nfs") || rest.contains("smbfs") || rest.contains("webdav")) {
                    return None;
                }
                let host = spec
                    .trim_start_matches("//")
                    .rsplit('@')
                    .next()?
                    .split([':', '/'])
                    .next()?
                    .to_string();
                Some((host, l))
            })
            .collect();
    }
    #[cfg(target_os = "windows")]
    {
        let out = Command::new("net").arg("use").output().ok();
        return out
            .into_iter()
            .flat_map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .filter_map(|l| {
                let unc = l.split_whitespace().find(|s| s.starts_with(r"\\"))?;
                let host = unc.trim_start_matches('\\').split('\\').next()?.to_string();
                Some((host, l))
            })
            .collect();
    }
    #[allow(unreachable_code)]
    Vec::new()
}

fn normalize_selected_root(root: PathBuf) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        let value = root.to_string_lossy();
        if let Some(path) = value.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{path}"));
        }
        if let Some(path) = value.strip_prefix(r"\\?\") {
            return PathBuf::from(path);
        }
    }
    root
}

fn same_root(left: &std::path::Path, right: &std::path::Path) -> bool {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        left.to_string_lossy()
            .trim_end_matches(['/', '\\'])
            .eq_ignore_ascii_case(right.to_string_lossy().trim_end_matches(['/', '\\']))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        left == right
    }
}

fn record_in_roots(path: &str, roots: &[PathBuf]) -> bool {
    let case_sensitive = cfg!(not(any(target_os = "windows", target_os = "macos")));
    roots
        .iter()
        .any(|root| portable_path_in_root(path, &root.to_string_lossy(), case_sensitive))
}

fn portable_path_in_root(path: &str, root: &str, case_sensitive: bool) -> bool {
    let normalize = |value: &str| {
        let value = value.replace('\\', "/");
        if case_sensitive {
            value
        } else {
            value.to_ascii_lowercase()
        }
    };
    let path = normalize(path);
    let mut root = normalize(root);
    while root.len() > 1 && root.ends_with('/') && !is_windows_drive_root(&root) {
        root.pop();
    }
    path == root
        || if root.ends_with('/') {
            path.starts_with(&root)
        } else {
            path.strip_prefix(&root)
                .is_some_and(|tail| tail.starts_with('/'))
        }
}

fn scope_within_selected_roots(scope: &str, selected_roots: &[PathBuf]) -> bool {
    let case_sensitive = cfg!(not(any(target_os = "windows", target_os = "macos")));
    selected_roots
        .iter()
        .any(|selected| portable_path_in_root(scope, &selected.to_string_lossy(), case_sensitive))
}

fn is_windows_drive_root(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() == 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'/'
}

fn default_system_roots() -> Vec<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::Storage::FileSystem::{
            GetDriveTypeW, GetLogicalDriveStringsW, DRIVE_FIXED, DRIVE_REMOVABLE,
        };

        let required = unsafe { GetLogicalDriveStringsW(0, std::ptr::null_mut()) };
        if required == 0 {
            return vec![PathBuf::from(r"C:\")];
        }
        let mut buffer = vec![0u16; required as usize + 1];
        let written = unsafe { GetLogicalDriveStringsW(buffer.len() as u32, buffer.as_mut_ptr()) };
        if written == 0 || written as usize >= buffer.len() {
            return vec![PathBuf::from(r"C:\")];
        }

        let mut roots = Vec::new();
        let mut offset = 0usize;
        while offset < written as usize {
            let Some(length) = buffer[offset..].iter().position(|value| *value == 0) else {
                break;
            };
            if length == 0 {
                break;
            }
            let root = &buffer[offset..offset + length + 1];
            offset += length + 1;
            let drive_type = unsafe { GetDriveTypeW(root.as_ptr()) };
            if matches!(drive_type, DRIVE_FIXED | DRIVE_REMOVABLE) {
                roots.push(PathBuf::from(String::from_utf16_lossy(&root[..length])));
            }
        }
        if roots.is_empty() {
            roots.push(PathBuf::from(r"C:\"));
        }
        return roots;
    }
    #[cfg(not(target_os = "windows"))]
    {
        vec![PathBuf::from("/")]
    }
}

fn selected_scan_mounts(roots: &[PathBuf]) -> Vec<MountInfo> {
    if roots.is_empty() {
        return Vec::new();
    }
    #[cfg(target_os = "linux")]
    {
        let trusted = neutra_core::mounts::system_mounts().unwrap_or_default();
        return select_mounts_for_roots(roots, &trusted);
    }
    #[cfg(target_os = "windows")]
    {
        let mut mounts = Vec::new();
        for root in roots {
            let value = root.to_string_lossy().replace('/', "\\");
            let bytes = value.as_bytes();
            if bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && bytes[2] == b'\\'
            {
                let mountpoint = PathBuf::from(&value[..3]);
                if !mounts
                    .iter()
                    .any(|mount: &MountInfo| mount.mountpoint == mountpoint)
                {
                    mounts.push(requested_mount(mountpoint));
                }
            }
        }
        return mounts;
    }
    #[cfg(target_os = "macos")]
    {
        let output = Command::new("/sbin/mount").output().ok();
        let trusted = output
            .filter(|output| output.status.success())
            .into_iter()
            .flat_map(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .filter_map(|line| {
                        let (_, mounted) = line.split_once(" on ")?;
                        let (mountpoint, options) = mounted.rsplit_once(" (")?;
                        let filesystem = options.trim_end_matches(')').split(',').next()?.trim();
                        let fs = match filesystem {
                            "apfs" | "hfs" => neutra_core::FsKind::Ext4,
                            "nfs" | "smbfs" | "webdav" => {
                                neutra_core::FsKind::Network(filesystem.into())
                            }
                            _ => return None,
                        };
                        Some(requested_mount_with_fs(PathBuf::from(mountpoint), fs))
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        return select_mounts_for_roots(roots, &trusted);
    }
    #[allow(unreachable_code)]
    Vec::new()
}

#[cfg(target_os = "windows")]
fn requested_mount(mountpoint: PathBuf) -> MountInfo {
    requested_mount_with_fs(mountpoint, neutra_core::FsKind::Ext4)
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn requested_mount_with_fs(mountpoint: PathBuf, fs: neutra_core::FsKind) -> MountInfo {
    MountInfo {
        device: String::new(),
        mountpoint,
        fs,
        source: neutra_core::MountSource::Local,
    }
}

#[cfg(any(not(target_os = "windows"), test))]
fn select_mounts_for_roots(roots: &[PathBuf], trusted: &[MountInfo]) -> Vec<MountInfo> {
    let mut selected = Vec::<MountInfo>::new();
    for root in roots {
        let Some(mount) = trusted
            .iter()
            .filter(|mount| root.starts_with(&mount.mountpoint))
            .max_by_key(|mount| mount.mountpoint.as_os_str().len())
        else {
            continue;
        };
        if mount.fs.is_indexable_local()
            && !selected
                .iter()
                .any(|existing| existing.mountpoint == mount.mountpoint)
        {
            selected.push(mount.clone());
        }
    }
    selected
}

fn gui_settings_path() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(std::env::temp_dir)
            .join("Neutrasearch/gui-settings.json")
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .map(|home| home.join("Library/Application Support"))
            .unwrap_or_else(std::env::temp_dir)
            .join("Neutrasearch/gui-settings.json")
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .filter(|path| path.is_absolute())
                    .map(|home| home.join(".config"))
            })
            .unwrap_or_else(std::env::temp_dir)
            .join("neutrasearch/gui-settings.json")
    }
}

fn load_gui_settings(path: &std::path::Path) -> Option<GuiSettings> {
    let bytes = std::fs::read(path).ok()?;
    let mut settings: GuiSettings = serde_json::from_slice(&bytes).ok()?;
    settings.roots = settings
        .roots
        .into_iter()
        .map(normalize_selected_root)
        .filter(|root| root.is_absolute())
        .collect();
    settings.roots.sort();
    settings
        .roots
        .dedup_by(|left, right| same_root(left, right));
    Some(settings)
}

fn save_gui_settings(path: &std::path::Path, settings: &GuiSettings) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "settings path has no parent".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create settings directory: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("cannot protect settings directory: {error}"))?;
    }
    let temporary = path.with_extension("json.new");
    let bytes = serde_json::to_vec_pretty(settings)
        .map_err(|error| format!("cannot encode settings: {error}"))?;
    let _ = std::fs::remove_file(&temporary);
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|error| format!("cannot create settings: {error}"))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("cannot write settings: {error}"))?;
    drop(file);
    publish_settings(&temporary, path).map_err(|error| format!("cannot publish settings: {error}"))
}

#[cfg(not(target_os = "windows"))]
fn publish_settings(
    temporary: &std::path::Path,
    destination: &std::path::Path,
) -> std::io::Result<()> {
    std::fs::rename(temporary, destination)
}

#[cfg(target_os = "windows")]
fn publish_settings(
    temporary: &std::path::Path,
    destination: &std::path::Path,
) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    extern "system" {
        fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
    }
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    let existing = temporary
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let replacement = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        MoveFileExW(
            existing.as_ptr(),
            replacement.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn env_flag(current: &str, legacy: &str) -> bool {
    std::env::var_os(current).is_some() || std::env::var_os(legacy).is_some()
}
fn configured_index() -> Option<PathBuf> {
    std::env::var_os("NEUTRASEARCH_INDEX")
        .or_else(|| std::env::var_os("NEUTRA_INDEX"))
        .map(PathBuf::from)
}
fn legacy_cache_path() -> PathBuf {
    if let Some(path) = configured_index() {
        return path;
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(std::env::temp_dir)
            .join("Neutrasearch/index.bin")
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .map(|home| home.join("Library/Caches"))
            .unwrap_or_else(std::env::temp_dir)
            .join("Neutrasearch/index.bin")
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .filter(|path| path.is_absolute())
                    .map(|home| home.join(".cache"))
            })
            .unwrap_or_else(std::env::temp_dir)
            .join("neutrasearch/index.bin")
    }
}
fn compact_cache_path() -> PathBuf {
    if let Some(path) = configured_index() {
        return path;
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(std::env::temp_dir)
            .join("Neutrasearch/index.nsx")
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .map(|home| home.join("Library/Application Support"))
            .unwrap_or_else(std::env::temp_dir)
            .join("Neutrasearch/index.nsx")
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .filter(|path| path.is_absolute())
                    .map(|home| home.join(".local/share"))
            })
            .unwrap_or_else(std::env::temp_dir)
            .join("neutrasearch/index.nsx")
    }
}

#[cfg(test)]
mod security_tests {
    use super::*;

    #[test]
    fn embedded_logo_decodes_to_a_complete_rgba_icon() {
        let icon = app_icon();
        assert_eq!((icon.width, icon.height), (128, 128));
        assert_eq!(icon.rgba.len(), 128 * 128 * 4);
    }

    #[test]
    fn elevated_helper_cannot_come_from_environment_override() {
        let error = select_helper(
            Some(PathBuf::from("/tmp/untrusted-helper")),
            Some(PathBuf::from("/usr/bin/neutrasearch")),
            true,
        )
        .unwrap_err();
        assert!(error.contains("refusing to elevate"));
    }

    #[test]
    #[cfg(unix)]
    fn elevated_helper_rejects_paths_outside_system_allowlist() {
        use std::os::unix::fs::PermissionsExt;
        let directory = std::env::temp_dir().join(format!(
            "neutrasearch-untrusted-helper-{}",
            std::process::id()
        ));
        let helper = directory.join("neutrasearch-helper");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(&helper, b"fixture").unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(validate_elevated_helper(&helper).is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn normal_helper_override_remains_available_for_development() {
        let helper = select_helper(
            Some(PathBuf::from("custom-helper")),
            Some(PathBuf::from("neutrasearch")),
            false,
        )
        .unwrap();
        assert_eq!(helper, PathBuf::from("custom-helper"));
    }

    #[test]
    fn missing_desktop_authorization_becomes_an_actionable_error() {
        let stderr = Arc::new(Mutex::new(
            "Error creating textual authentication agent: Error opening current controlling terminal"
                .to_owned(),
        ));
        let error = helper_start_failure("handshake failed", &stderr);
        assert!(error.contains("launch Neutrasearch from the desktop"));
        assert!(!error.contains("handshake"));
    }

    #[test]
    fn selected_roots_include_descendants_but_not_prefix_siblings() {
        let roots = vec![PathBuf::from("/home/alex/Documents")];
        assert!(record_in_roots("/home/alex/Documents/report.pdf", &roots));
        assert!(record_in_roots("/home/alex/Documents", &roots));
        assert!(!record_in_roots(
            "/home/alex/Documents-old/report.pdf",
            &roots
        ));
        assert!(!record_in_roots("/home/alex/Documents/report.pdf", &[]));
        assert!(portable_path_in_root("/Users/alex/report.pdf", "/", false));
        assert!(portable_path_in_root(
            r"C:\Users\alex\report.pdf",
            r"C:\",
            false
        ));
        assert!(!portable_path_in_root(r"D:\report.pdf", r"C:\", false));
        assert!(scope_within_selected_roots(
            "/home/alex/Documents/reports",
            &[PathBuf::from("/home/alex/Documents")]
        ));
        assert!(!scope_within_selected_roots(
            "/home",
            &[PathBuf::from("/home/alex/Documents")]
        ));
    }

    #[test]
    fn network_roots_do_not_fall_back_to_scanning_the_parent_local_volume() {
        let trusted = vec![
            MountInfo {
                device: "server:/share".into(),
                mountpoint: "/mnt/team".into(),
                fs: neutra_core::FsKind::Network("nfs4".into()),
                source: neutra_core::MountSource::Remote {
                    host: "server".into(),
                },
            },
            MountInfo {
                device: "/dev/root".into(),
                mountpoint: "/".into(),
                fs: neutra_core::FsKind::Ext4,
                source: neutra_core::MountSource::Local,
            },
        ];
        assert!(select_mounts_for_roots(&[PathBuf::from("/mnt/team/docs")], &trusted).is_empty());
        let local = select_mounts_for_roots(&[PathBuf::from("/home/alex")], &trusted);
        assert_eq!(local.len(), 1);
        assert_eq!(local[0].mountpoint, PathBuf::from("/"));
    }

    #[test]
    fn successful_empty_scans_still_replace_stale_results() {
        assert!(scan_has_reachable_lane(1, 0));
        assert!(scan_has_reachable_lane(3, 1));
        assert!(!scan_has_reachable_lane(3, 3));
        assert!(!scan_has_reachable_lane(0, 0));
    }

    #[test]
    fn offline_network_errors_are_retryable_but_integrity_failures_are_not() {
        assert!(remote_failure_is_offline(&anyhow::anyhow!(
            "ssh: connect to host studio: Connection timed out"
        )));
        assert!(!remote_failure_is_offline(&anyhow::anyhow!(
            "helper checksum mismatch"
        )));
        assert!(!remote_failure_is_offline(&anyhow::anyhow!(
            "Permission denied (publickey)"
        )));
    }

    #[test]
    fn gui_settings_roundtrip_preserves_completed_onboarding_and_roots() {
        let directory =
            std::env::temp_dir().join(format!("neutrasearch-gui-settings-{}", std::process::id()));
        let path = directory.join("gui-settings.json");
        let _ = std::fs::remove_dir_all(&directory);
        let root = std::env::temp_dir();
        let settings = GuiSettings {
            onboarding_complete: true,
            roots: vec![root.clone()],
        };
        save_gui_settings(&path, &settings).unwrap();
        let incomplete = GuiSettings {
            onboarding_complete: false,
            roots: vec![root.clone()],
        };
        save_gui_settings(&path, &incomplete).unwrap();
        save_gui_settings(&path, &settings).unwrap();
        let loaded = load_gui_settings(&path).unwrap();
        assert!(loaded.onboarding_complete);
        assert_eq!(loaded.roots, vec![root]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        std::fs::remove_dir_all(directory).unwrap();
    }
}
