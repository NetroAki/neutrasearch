use eframe::egui;
use egui::{Color32, FontId, RichText, Stroke};
use egui_expressive::widgets::SearchField;
use egui_expressive::{M3Theme, StatusBar, StatusBarItem, Theme};
use neutra_core::proto::{read_frame, write_frame, ClientMsg, HelperMsg, PROTO_VERSION};
use neutra_core::{CompactIndex, FileKind, Index, Query, SearchHit, SearchStats};
use std::collections::BTreeMap;
use std::io::{BufReader, BufWriter};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

const INK: Color32 = Color32::from_rgb(8, 11, 15);
const PANEL: Color32 = Color32::from_rgb(15, 20, 27);
const RAISED: Color32 = Color32::from_rgb(23, 30, 39);
const TEXT: Color32 = Color32::from_rgb(225, 235, 239);
const MUTED: Color32 = Color32::from_rgb(118, 135, 143);
const ACID: Color32 = Color32::from_rgb(126, 241, 187);
const BLUE: Color32 = Color32::from_rgb(93, 170, 255);
const WARN: Color32 = Color32::from_rgb(255, 184, 92);

enum Event {
    Message(HelperMsg),
    Fatal(String),
    Remote {
        key: String,
        status: String,
        error: bool,
    },
    CompactReady(CompactIndex),
    CompactFailed(String, Index),
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
    treemap_height: f32,
    treemap_open: bool,
    about_open: bool,
}

fn main() -> eframe::Result<()> {
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
        apply_theme(&cc.egui_ctx);
        let cache_path = compact_cache_path();
        let compact = CompactIndex::open(&cache_path).ok();
        let restored = if compact.is_none() {
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
            query: String::new(),
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
            treemap_height: 170.0,
            treemap_open: true,
            about_open: false,
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
        if std::env::var_os("NEUTRA_NO_REMOTE").is_none() {
            spawn_network_watcher(app.tx.clone());
        }
        if !has_durable_index && std::env::var_os("NEUTRA_NO_AUTOSCAN").is_none()
            || std::env::var_os("NEUTRA_FORCE_RESCAN").is_some()
        {
            app.begin_scan();
        }
        app
    }

    fn begin_scan(&mut self) {
        if self.scanning {
            return;
        }
        self.compact = None;
        self.index = Index::new();
        self.requery();
        self.scanning = true;
        self.active_scans = 0;
        self.lanes.clear();
        spawn_local_helper(self.tx.clone());
    }
    fn process_events(&mut self) {
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                Event::Fatal(e) => {
                    self.scanning = false;
                    self.active_scans = 0;
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
                Event::CompactFailed(error, index) => {
                    self.index = index;
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
                        self.index.extend(records);
                        self.cache_dirty = true;
                    }
                    HelperMsg::ScanDone { mount, stats } => {
                        self.active_scans = self.active_scans.saturating_sub(1);
                        self.scanning = self.active_scans > 0;
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
                        self.scanning = self.active_scans > 0;
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
                    HelperMsg::Error(e) => {
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
                },
            }
        }
        let generation = self.index.generation();
        if generation != self.last_generation {
            self.last_generation = generation;
            self.requery();
        }
        if self.cache_dirty
            && !self.building_cache
            && self.active_scans == 0
            && self.last_cache.elapsed() > Duration::from_secs(2)
        {
            let index = std::mem::replace(&mut self.index, Index::new());
            let path = self.cache_path.clone();
            let tx = self.tx.clone();
            self.building_cache = true;
            self.lanes.insert(
                "cache".into(),
                LaneState {
                    label: "COMPACT INDEX".into(),
                    status: "building compressed search blocks".into(),
                    records: index.len() as u64,
                    ..Default::default()
                },
            );
            std::thread::spawn(move || {
                match CompactIndex::build(index.records(), &path)
                    .and_then(|_| CompactIndex::open(&path))
                {
                    Ok(compact) => {
                        let _ = tx.send(Event::CompactReady(compact));
                    }
                    Err(error) => {
                        let _ = tx.send(Event::CompactFailed(error.to_string(), index));
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
    fn requery(&mut self) {
        if self.query.trim().is_empty() {
            self.hits.clear();
            self.search_stats = SearchStats {
                scanned: self.index_len(),
                matched: self.index_len(),
                wall_us: 0,
            };
            return;
        }
        let mut q = Query::parse(&self.query);
        q.limit = 500;
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

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.set_width(220.0);
        ui.add_space(18.0);
        ui.label(RichText::new("NEUTRA").size(11.0).color(ACID).strong());
        ui.label(RichText::new("SEARCH//").size(25.0).color(TEXT).strong());
        ui.label(
            RichText::new("NATIVE METADATA LANES")
                .size(9.0)
                .color(MUTED),
        );
        ui.add_space(22.0);
        if ui
            .add_sized(
                [190.0, 34.0],
                egui::Button::new(if self.scanning {
                    "SCANNING…"
                } else {
                    "RESCAN NATIVE INDEX"
                })
                .fill(RAISED)
                .stroke(Stroke::new(1.0_f32, ACID)),
            )
            .clicked()
        {
            self.begin_scan();
        }
        ui.add_space(18.0);
        ui.label(RichText::new("LANES").size(10.0).color(MUTED).strong());
        ui.add_space(6.0);
        egui::ScrollArea::vertical().show(ui, |ui| {
            for lane in self.lanes.values() {
                let c = if lane.error { WARN } else { ACID };
                egui::Frame::new()
                    .fill(RAISED)
                    .corner_radius(5)
                    .inner_margin(egui::Margin::same(9))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.colored_label(c, "●");
                            ui.label(RichText::new(&lane.label).size(11.0).strong());
                        });
                        if lane.records > 0 {
                            ui.label(
                                RichText::new(format!(
                                    "{} entries · {} ms",
                                    fmt_count(lane.records),
                                    lane.ms
                                ))
                                .size(10.0)
                                .color(MUTED),
                            );
                        } else {
                            ui.label(
                                RichText::new(shorten(&lane.status, 56))
                                    .size(10.0)
                                    .color(MUTED),
                            );
                        }
                    });
                ui.add_space(6.0);
            }
        });
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, 0.0, PANEL);
        // Neutra's asymmetric scanline is deliberately not a default egui header.
        ui.painter().rect_filled(
            egui::Rect::from_min_size(rect.left_top(), egui::vec2(5.0, rect.height())),
            0.0,
            ACID,
        );
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            ui.add_space(18.0);
            ui.vertical(|ui| {
                ui.label(
                    RichText::new("WHOLE-NAMESPACE INDEX")
                        .size(9.0)
                        .color(ACID)
                        .strong(),
                );
                ui.label(
                    RichText::new("Find without walking.")
                        .size(19.0)
                        .color(TEXT)
                        .strong(),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(22.0);
                if ui
                    .add(
                        egui::Button::new(RichText::new("ABOUT").size(10.0).color(ACID))
                            .fill(RAISED)
                            .stroke(Stroke::new(1.0_f32, Color32::from_rgb(45, 61, 69))),
                    )
                    .clicked()
                {
                    self.about_open = true;
                }
                ui.add_space(10.0);
                ui.label(
                    RichText::new(format!("{} OBJECTS", fmt_count(self.index_len())))
                        .size(11.0)
                        .color(MUTED),
                );
            });
        });
        ui.add_space(12.0);
    }

    fn about_window(&mut self, ctx: &egui::Context) {
        if !self.about_open {
            return;
        }
        let mut open = true;
        egui::Window::new("About Neutrasearch")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(420.0)
            .show(ctx, |ui| {
                ui.label(
                    RichText::new("NEUTRASEARCH//")
                        .size(24.0)
                        .color(ACID)
                        .strong(),
                );
                ui.label(
                    RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION")))
                        .monospace()
                        .color(MUTED),
                );
                ui.add_space(12.0);
                ui.label("Fast native-metadata filename search without directory walking.");
                ui.label("Created by NetroAki. Released under the MIT License.");
                ui.add_space(12.0);
                ui.label(
                    RichText::new("SUPPORT DEVELOPMENT")
                        .size(10.0)
                        .color(MUTED)
                        .strong(),
                );
                ui.hyperlink_to("Ko-fi · ko-fi.com/netroaki", "https://ko-fi.com/netroaki");
                ui.hyperlink_to(
                    "Patreon · patreon.com/NetroAki",
                    "https://www.patreon.com/NetroAki",
                );
                ui.add_space(10.0);
                ui.hyperlink_to(
                    "Source · github.com/NetroAki/neutrasearch",
                    "https://github.com/NetroAki/neutrasearch",
                );
            });
        self.about_open = open;
    }

    fn search_bar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            ui.add_space(18.0);
            ui.label(RichText::new("⌕").size(24.0).color(ACID));
            let before = self.query.clone();
            ui.add_sized(
                [ui.available_width() - 180.0, 42.0],
                SearchField::new(&mut self.query),
            );
            if before != self.query {
                self.requery();
            }
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!(
                    "{} / {} µs",
                    fmt_count(self.search_stats.matched),
                    self.search_stats.wall_us
                ))
                .size(10.0)
                .color(MUTED),
            );
            ui.add_space(18.0);
        });
        ui.add_space(12.0);
    }

    fn size_map(&mut self, ui: &mut egui::Ui) {
        let handle_h = 24.0;
        let (handle, response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), handle_h),
            egui::Sense::click_and_drag(),
        );
        ui.painter().rect_filled(handle, 0.0, PANEL);
        ui.painter().hline(
            (handle.center().x - 24.0)..=(handle.center().x + 24.0),
            handle.center().y,
            Stroke::new(2.0_f32, MUTED),
        );
        ui.painter().text(
            handle.left_center() + egui::vec2(14.0, 0.0),
            egui::Align2::LEFT_CENTER,
            if self.treemap_open {
                "SIZE MAP  ▾"
            } else {
                "SIZE MAP  ▴"
            },
            FontId::monospace(10.0),
            ACID,
        );
        if response.clicked() {
            self.treemap_open = !self.treemap_open;
        }
        if response.dragged() {
            self.treemap_open = true;
            self.treemap_height =
                (self.treemap_height - response.drag_delta().y).clamp(90.0, 420.0);
        }
        if !self.treemap_open {
            return;
        }
        let map_h = (self.treemap_height - handle_h).max(60.0);
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), map_h),
            egui::Sense::hover(),
        );
        ui.painter()
            .rect_filled(rect, 0.0, Color32::from_rgb(6, 9, 12));
        let mut groups = std::collections::HashMap::<String, (u64, u64)>::new();
        for hit in &self.hits {
            if hit.record.kind != FileKind::File {
                continue;
            }
            let ext = match hit.record.extension() {
                "" => "(none)",
                x => x,
            };
            let g = groups.entry(ext.to_ascii_lowercase()).or_default();
            g.0 = g.0.saturating_add(hit.record.size.max(1));
            g.1 += 1;
        }
        let mut groups = groups
            .into_iter()
            .map(|(name, (bytes, count))| MapBlock { name, bytes, count })
            .collect::<Vec<_>>();
        groups.sort_unstable_by(|a, b| b.bytes.cmp(&a.bytes));
        groups.truncate(96);
        let mut laid = Vec::new();
        layout_map(&groups, rect.shrink(3.0), &mut laid);
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let mut clicked_ext = None;
        for (block, r) in laid {
            let color = map_color(&block.name);
            let hovered = pointer.is_some_and(|p| r.contains(p));
            ui.painter().rect_filled(
                r.shrink(1.0),
                2.0,
                if hovered {
                    color
                } else {
                    color.gamma_multiply(0.72)
                },
            );
            if r.width() > 54.0 && r.height() > 30.0 {
                ui.painter().text(
                    r.left_top() + egui::vec2(6.0, 5.0),
                    egui::Align2::LEFT_TOP,
                    &block.name,
                    FontId::monospace(10.0),
                    Color32::WHITE,
                );
                ui.painter().text(
                    r.left_bottom() + egui::vec2(6.0, -5.0),
                    egui::Align2::LEFT_BOTTOM,
                    format!("{} · {}", format_size(block.bytes), block.count),
                    FontId::monospace(9.0),
                    Color32::from_white_alpha(190),
                );
            }
            if hovered && ui.input(|i| i.pointer.primary_clicked()) {
                clicked_ext = Some(block.name.clone());
            }
        }
        if let Some(ext) = clicked_ext {
            if ext == "(none)" {
                self.query = "ext:".into();
            } else {
                self.query = format!("ext:{ext}");
            }
            self.requery();
        }
    }

    fn results(&mut self, ui: &mut egui::Ui) {
        let row_h = 52.0;
        let count = self.hits.len();
        if count == 0 {
            ui.centered_and_justified(|ui| {
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("NO SIGNAL").size(26.0).color(MUTED).strong());
                    ui.label(
                        RichText::new(if self.index_is_empty() {
                            "Waiting for a native index lane…"
                        } else {
                            "Try a name, extension, kind, filesystem, size, or under: filter."
                        })
                        .color(MUTED),
                    );
                });
            });
            return;
        }
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_rows(ui, row_h, count, |ui, range| {
                for i in range {
                    let hit = &self.hits[i];
                    let rec = &hit.record;
                    let (r, response) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), row_h),
                        egui::Sense::click(),
                    );
                    let selected = self.selected.as_deref() == Some(rec.path.as_ref());
                    let fill = if selected {
                        Color32::from_rgb(29, 48, 52)
                    } else if response.hovered() {
                        Color32::from_rgb(22, 31, 39)
                    } else if i % 2 == 0 {
                        INK
                    } else {
                        Color32::from_rgb(11, 15, 20)
                    };
                    ui.painter().rect_filled(r, 0.0, fill);
                    if response.hovered() || selected {
                        ui.painter().rect_filled(
                            egui::Rect::from_min_size(r.left_top(), egui::vec2(3.0, r.height())),
                            0.0,
                            ACID,
                        );
                    }
                    let icon = match rec.kind {
                        FileKind::Dir => "D",
                        FileKind::Symlink => "L",
                        FileKind::File => "F",
                        FileKind::Other => "?",
                    };
                    let ic = match rec.kind {
                        FileKind::Dir => BLUE,
                        FileKind::Symlink => WARN,
                        _ => ACID,
                    };
                    let center = r.left_center() + egui::vec2(25.0, 0.0);
                    ui.painter()
                        .circle_filled(center, 13.0, ic.gamma_multiply(0.16));
                    ui.painter().text(
                        center,
                        egui::Align2::CENTER_CENTER,
                        icon,
                        FontId::monospace(10.0),
                        ic,
                    );
                    let name_pos = r.left_top() + egui::vec2(50.0, 9.0);
                    ui.painter().text(
                        name_pos,
                        egui::Align2::LEFT_TOP,
                        rec.name(),
                        FontId::proportional(14.0),
                        TEXT,
                    );
                    ui.painter().text(
                        name_pos + egui::vec2(0.0, 21.0),
                        egui::Align2::LEFT_TOP,
                        shorten(&rec.path, 110),
                        FontId::monospace(10.5),
                        MUTED,
                    );
                    ui.painter().text(
                        r.right_center() - egui::vec2(18.0, 8.0),
                        egui::Align2::RIGHT_CENTER,
                        format!(
                            "{}  ·  {}",
                            format_size(rec.size),
                            rec.fs.label().to_uppercase()
                        ),
                        FontId::monospace(10.0),
                        MUTED,
                    );
                    if response.clicked() {
                        self.selected = Some(rec.path.to_string());
                    }
                }
            });
    }
}

impl eframe::App for NeutraApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.process_events();
        ui.ctx().request_repaint_after(Duration::from_millis(100));
        let size = ui.available_size();
        ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
        ui.horizontal(|ui| {
            egui::Frame::new()
                .fill(PANEL)
                .inner_margin(egui::Margin::same(12))
                .show(ui, |ui| {
                    ui.set_width(196.0);
                    ui.set_min_height(size.y);
                    self.sidebar(ui);
                });
            ui.vertical(|ui| {
                ui.set_width(ui.available_width());
                ui.allocate_ui(egui::vec2(ui.available_width(), 84.0), |ui| self.header(ui));
                ui.allocate_ui(egui::vec2(ui.available_width(), 72.0), |ui| {
                    self.search_bar(ui)
                });
                let status_h = 30.0;
                let map_h = if self.treemap_open {
                    self.treemap_height
                } else {
                    24.0
                };
                let results_h = (ui.available_height() - status_h - map_h).max(80.0);
                ui.allocate_ui(egui::vec2(ui.available_width(), results_h), |ui| {
                    egui::Frame::new().fill(INK).show(ui, |ui| {
                        ui.set_min_height(results_h);
                        self.results(ui);
                    });
                });
                ui.allocate_ui(egui::vec2(ui.available_width(), map_h), |ui| {
                    self.size_map(ui)
                });
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::symmetric(12, 4))
                    .show(ui, |ui| {
                        let items = [
                            StatusBarItem::new("INDEX").value(fmt_count(self.index_len())),
                            StatusBarItem::new("MATCHED")
                                .value(fmt_count(self.search_stats.matched)),
                            StatusBarItem::new("SEARCH")
                                .value(format!("{} µs", self.search_stats.wall_us)),
                            StatusBarItem::new("MODE").value("NATIVE / NO WALK"),
                        ];
                        ui.add(StatusBar::new(&items));
                    });
            });
        });
        self.about_window(ui.ctx());
    }
}

fn spawn_local_helper(tx: Sender<Event>) {
    std::thread::spawn(move || {
        let helper = std::env::var_os("NEUTRA_HELPER")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::current_exe().ok().map(|p| {
                    p.with_file_name(if cfg!(windows) {
                        "neutra-helper.exe"
                    } else {
                        "neutra-helper"
                    })
                })
            })
            .unwrap_or_else(|| PathBuf::from("neutra-helper"));
        let mut cmd = if cfg!(target_os = "linux") && std::env::var_os("NEUTRA_PKEXEC").is_some() {
            let mut c = Command::new("pkexec");
            c.arg(helper);
            c
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
                let _ = tx.send(Event::Fatal(format!("cannot start neutra-helper: {e}")));
                return;
            }
        };
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

fn apply_theme(ctx: &egui::Context) {
    let theme = Theme::dark();
    theme.store(ctx);
    let m3 = M3Theme::from_seed(ACID, true);
    m3.store(ctx);
    let mut v = egui::Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = RAISED;
    v.extreme_bg_color = INK;
    v.selection.bg_fill = Color32::from_rgb(31, 72, 60);
    v.selection.stroke = Stroke::new(1.0_f32, ACID);
    v.widgets.inactive.bg_fill = RAISED;
    v.widgets.hovered.bg_fill = Color32::from_rgb(31, 41, 50);
    v.widgets.active.bg_fill = Color32::from_rgb(35, 65, 59);
    v.override_text_color = Some(TEXT);
    ctx.set_visuals(v);
    let mut s = (*ctx.global_style()).clone();
    s.spacing.item_spacing = egui::vec2(8.0, 7.0);
    s.spacing.interact_size.y = 34.0;
    s.visuals.widgets.inactive.corner_radius = 5.into();
    ctx.set_global_style(s);
}
fn legacy_cache_path() -> PathBuf {
    if let Some(p) = std::env::var_os("NEUTRA_INDEX") {
        return p.into();
    }
    #[cfg(target_os = "windows")]
    {
        return std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join("Neutrasearch/index.bin");
    }
    #[cfg(target_os = "macos")]
    {
        return std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join("Library/Caches/Neutrasearch/index.bin");
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
            .unwrap_or_default()
            .join("neutrasearch/index.bin")
    }
}
fn compact_cache_path() -> PathBuf {
    if let Some(path) = std::env::var_os("NEUTRA_INDEX") {
        return path.into();
    }
    let mut path = legacy_cache_path();
    path.set_extension("nsx");
    path
}
#[derive(Clone)]
struct MapBlock {
    name: String,
    bytes: u64,
    count: u64,
}
fn layout_map<'a>(
    items: &'a [MapBlock],
    rect: egui::Rect,
    out: &mut Vec<(&'a MapBlock, egui::Rect)>,
) {
    if items.is_empty() || rect.width() < 2.0 || rect.height() < 2.0 {
        return;
    }
    if items.len() == 1 {
        out.push((&items[0], rect));
        return;
    }
    let total = items.iter().map(|b| b.bytes).sum::<u64>().max(1);
    let mut left = 0u64;
    let mut split = 1usize;
    for (i, b) in items.iter().enumerate().take(items.len() - 1) {
        left = left.saturating_add(b.bytes);
        split = i + 1;
        if left >= total / 2 {
            break;
        }
    }
    let ratio = (left as f32 / total as f32).clamp(0.08, 0.92);
    let (a, b) = if rect.width() >= rect.height() {
        let x = rect.left() + rect.width() * ratio;
        (
            egui::Rect::from_min_max(rect.min, egui::pos2(x, rect.bottom())),
            egui::Rect::from_min_max(egui::pos2(x, rect.top()), rect.max),
        )
    } else {
        let y = rect.top() + rect.height() * ratio;
        (
            egui::Rect::from_min_max(rect.min, egui::pos2(rect.right(), y)),
            egui::Rect::from_min_max(egui::pos2(rect.left(), y), rect.max),
        )
    };
    layout_map(&items[..split], a, out);
    layout_map(&items[split..], b, out);
}
fn map_color(name: &str) -> Color32 {
    let h = name.bytes().fold(0x811c9dc5u32, |a, b| {
        a.wrapping_mul(16_777_619) ^ (b as u32)
    });
    let colors = [
        Color32::from_rgb(43, 133, 117),
        Color32::from_rgb(43, 101, 154),
        Color32::from_rgb(142, 83, 145),
        Color32::from_rgb(177, 108, 56),
        Color32::from_rgb(89, 126, 65),
        Color32::from_rgb(141, 65, 79),
    ];
    colors[(h as usize) % colors.len()]
}
fn fmt_count(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.2}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}
fn format_size(n: u64) -> String {
    if n >= 1 << 30 {
        format!("{:.1}G", n as f64 / (1u64 << 30) as f64)
    } else if n >= 1 << 20 {
        format!("{:.1}M", n as f64 / (1u64 << 20) as f64)
    } else if n >= 1 << 10 {
        format!("{:.1}K", n as f64 / (1u64 << 10) as f64)
    } else {
        format!("{n}B")
    }
}
fn shorten(s: &str, max: usize) -> String {
    let chars = s.chars().count();
    if chars <= max {
        return s.into();
    }
    let tail = s
        .chars()
        .rev()
        .take(max.saturating_sub(1))
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("…{tail}")
}
