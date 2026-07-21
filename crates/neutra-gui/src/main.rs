mod terminal;
mod ui;

use eframe::egui;
use neutra_core::proto::{read_frame, write_frame, ClientMsg, HelperMsg, PROTO_VERSION};
use neutra_core::{
    CompactIndex, FileKind, FileRecord, Index, Query, SearchHit, SearchStats, SortKey,
};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, BufWriter};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
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

struct NeutraApp {
    index: Index,
    compact: Option<CompactIndex>,
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
            .with_title("Neutrasearch"),
        ..Default::default()
    };
    eframe::run_native(
        "Neutrasearch",
        options,
        Box::new(|cc| Ok(Box::new(NeutraApp::new(cc)))),
    )
}

impl NeutraApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        ui::configure(&cc.egui_ctx);
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
        let (tx, rx) = mpsc::channel();
        let mut app = Self {
            index,
            compact,
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
            regex_mode: false,
            scope_root: None,
            diagnostics_open: false,
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
                    status: "Choose Build search index to approve native local-volume indexing"
                        .into(),
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
        if !reference_mode
            && (env_flag("NEUTRASEARCH_AUTOSCAN", "NEUTRA_AUTOSCAN")
                || env_flag("NEUTRASEARCH_FORCE_RESCAN", "NEUTRA_FORCE_RESCAN"))
        {
            app.begin_scan();
        }
        app
    }

    fn begin_scan(&mut self) {
        if self.scanning || self.building_cache {
            return;
        }
        // Build into a staging index. The last complete index remains searchable
        // and on disk until every requested native lane succeeds.
        self.scan_index = Some(Index::new());
        self.scanning = true;
        self.active_scans = 0;
        self.cache_dirty = false;
        self.lanes.clear();
        spawn_local_helper(self.tx.clone());
    }
    fn process_events(&mut self) {
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                Event::Fatal(e) => {
                    self.scanning = false;
                    self.active_scans = 0;
                    self.scan_index = None;
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
                        if let Some(staging) = &mut self.scan_index {
                            staging.extend(records);
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
                        if mounts == 0 {
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
                        } else if errors > 0 {
                            self.lanes.insert(
                                "scan".into(),
                                LaneState {
                                    label: "REBUILD NOT PUBLISHED".into(),
                                    status: format!(
                                        "{errors} of {mounts} native lanes failed; keeping the last complete index"
                                    ),
                                    error: true,
                                    ..Default::default()
                                },
                            );
                        } else if let Some(staging) = staging {
                            self.compact = None;
                            self.index = staging;
                            self.tree_model = None;
                            self.tree_building = false;
                            self.cache_dirty = true;
                            self.last_cache = Instant::now() - Duration::from_secs(3);
                            self.requery();
                        }
                    }
                    HelperMsg::Error(e) => {
                        self.scanning = false;
                        self.active_scans = 0;
                        self.scan_index = None;
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
            let path = self.cache_path.clone();
            let tx = self.tx.clone();
            self.building_cache = true;
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
        if let Some(root) = &self.scope_root {
            q.scope_roots.push(root.clone());
            q.scope_case_sensitive = cfg!(not(any(target_os = "windows", target_os = "macos")));
        }
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
            "refusing to elevate a helper selected through NEUTRASEARCH_HELPER; install trusted sibling binaries"
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
        let helper = sibling.ok_or_else(|| "cannot locate installed helper sibling".to_string())?;
        return validate_elevated_helper(&helper);
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

fn spawn_local_helper(tx: Sender<Event>) {
    std::thread::spawn(move || {
        let configured = std::env::var_os("NEUTRASEARCH_HELPER")
            .or_else(|| std::env::var_os("NEUTRA_HELPER"))
            .map(PathBuf::from);
        let elevated = cfg!(target_os = "linux")
            && (std::env::var_os("NEUTRASEARCH_PKEXEC").is_some()
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
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(Event::Fatal(format!(
                    "cannot start neutrasearch-helper: {e}"
                )));
                return;
            }
        };
        if let Some(stderr) = child.stderr.take() {
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    eprintln!("neutrasearch-helper: {line}");
                }
            });
        }
        let (mut input, mut output) = (
            BufWriter::new(child.stdin.take().unwrap()),
            BufReader::new(child.stdout.take().unwrap()),
        );
        if write_frame(
            &mut input,
            &ClientMsg::Hello {
                proto: PROTO_VERSION,
            },
        )
        .is_err()
        {
            return;
        }
        match read_frame::<_, HelperMsg>(&mut output) {
            Ok(Some(h)) => {
                let _ = tx.send(Event::Message(h));
            }
            other => {
                let _ = tx.send(Event::Fatal(format!("helper handshake failed: {other:?}")));
                return;
            }
        }
        if write_frame(&mut input, &ClientMsg::Scan { mounts: Vec::new() }).is_err() {
            return;
        }
        drop(input);
        loop {
            match read_frame::<_, HelperMsg>(&mut output) {
                Ok(Some(m)) => {
                    let _ = tx.send(Event::Message(m));
                }
                Ok(None) => break,
                Err(e) => {
                    let _ = tx.send(Event::Fatal(format!("helper protocol: {e}")));
                    break;
                }
            }
        }
        let _ = child.wait();
    });
}

fn spawn_network_watcher(tx: Sender<Event>) {
    std::thread::spawn(move || {
        let provisioner = neutra_remote::Provisioner::from_env();
        let mut seen = std::collections::HashSet::<String>::new();
        loop {
            for (host, key) in discover_network_hosts() {
                if !seen.insert(key.clone()) {
                    continue;
                }
                let result = provisioner.ensure_installed(&host);
                let (status, error) = match result {
                    Ok(p) => (
                        format!(
                            "helper build {} ready ({:?}/{})",
                            neutra_core::proto::HELPER_BUILD,
                            p.os,
                            p.arch
                        ),
                        false,
                    ),
                    Err(e) => (format!("auto-install unavailable: {e:#}"), true),
                };
                let _ = tx.send(Event::Remote { key, status, error });
            }
            std::thread::sleep(Duration::from_secs(3));
        }
    });
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
        return std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(std::env::temp_dir)
            .join("Neutrasearch/index.bin");
    }
    #[cfg(target_os = "macos")]
    {
        return std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .map(|home| home.join("Library/Caches"))
            .unwrap_or_else(std::env::temp_dir)
            .join("Neutrasearch/index.bin");
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
        return std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(std::env::temp_dir)
            .join("Neutrasearch/index.nsx");
    }
    #[cfg(target_os = "macos")]
    {
        return std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .map(|home| home.join("Library/Application Support"))
            .unwrap_or_else(std::env::temp_dir)
            .join("Neutrasearch/index.nsx");
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
    fn normal_helper_override_remains_available_for_development() {
        let helper = select_helper(
            Some(PathBuf::from("custom-helper")),
            Some(PathBuf::from("neutrasearch")),
            false,
        )
        .unwrap();
        assert_eq!(helper, PathBuf::from("custom-helper"));
    }
}
