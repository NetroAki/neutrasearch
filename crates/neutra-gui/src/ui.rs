use super::*;
use egui::{
    Align, Align2, Color32, FontData, FontDefinitions, FontFamily, FontId, Id, Layout, Margin,
    Rect, RichText, Sense, Stroke, StrokeKind, TextStyle, Ui, Vec2,
};
use egui_expressive::widgets::SearchField;
use egui_expressive::{ResizableSplit, SplitAxis, Theme};
use regex::RegexBuilder;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use neutra_core::{FileKind, FileRecord};
use std::time::Duration;

mod dialogs;
mod hierarchy;
mod icons;
mod results;
mod sidebar;
mod theme;
mod tree_panel;
 mod treemap;
 pub(super) mod widgets;

use icons::filter_tab;
pub(crate) use sidebar::SidebarTab;
use widgets::{
    ancestor_paths,
     extension_color, fixed_strip, fmt_count, format_mtime, format_size, mono,
    parent_path, path_name, primary_button, sans, secondary_button, segment_button, shorten,
    type_badge, type_color, ACID, ACID_STRONG, ACTIVE, BLACK, VIOLET, CANVAS, ERROR, HOVER,
    LINE, LINE_STRONG, MUTED, RAISED, SUBTLE, SURFACE, TEXT, SELECTED, GLOW, WARN, CAPTION, SMALL, MICRO, tracked, bar_style, ghost_style,
};
use widgets::{copy_to_clipboard, paint_search_icon, task_icon};
use dialogs::{about_dialog, diagnostics_dialog};
use sidebar::banner::{banner_color, runtime_banner};

use results::{details_view, grid_view, list_view, perform_file_action, surrender_widget_focus};
use treemap::treemap_view;
 pub(super) use hierarchy::Hierarchy;

const MENU_H: f32 = 30.0;
const QUERY_H: f32 = 44.0;
const FILTER_H: f32 = 34.0;
const TOOLBAR_H: f32 = 38.0;
const BANNER_H: f32 = 44.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum ResultView {
    #[default]
    Details,
    List,
    Grid,
    Treemap,
}

impl ResultView {
    const ALL: [Self; 4] = [Self::Details, Self::List, Self::Grid, Self::Treemap];

    fn label(self) -> &'static str {
        match self {
            Self::Details => "Details",
            Self::List => "List",
            Self::Grid => "Grid",
            Self::Treemap => "Treemap",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum KindFilter {
    #[default]
    All,
    Audio,
    Images,
    Video,
    Programs,
    Compressed,
    Documents,
    Folders,
    /// Kept so older settings still load; the button row no longer offers
    /// it, and startup migrates it to `All` (see app state).
    Files,
}

impl KindFilter {
    const ALL: [Self; 8] = [
        Self::All,
        Self::Audio,
        Self::Images,
        Self::Video,
        Self::Programs,
        Self::Compressed,
        Self::Documents,
        Self::Folders,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Audio => "Audio",
            Self::Images => "Images",
            Self::Video => "Video",
            Self::Programs => "Programs",
            Self::Compressed => "Archives",
            Self::Documents => "Documents",
            Self::Folders => "Folders",
            Self::Files => "Files",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum SortMode {
    /// Engine relevance (name-prefix > name > path, then mtime). With no
    /// query text every score is equal, so this degrades to newest-first.
    #[default]
    Relevance,
    Modified,
    Name,
    Size,
    Path,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum SearchMode {
    /// File/folder names only. The default: directory paths never match
    /// unless the user picks one of the path options below.
    #[default]
    Name,
    NameAndPath,
    Path,
}

impl SearchMode {
    fn label(self) -> &'static str {
        match self {
            Self::Name => "File names only",
            Self::NameAndPath => "Names + folder paths",
            Self::Path => "Folder paths only",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RuntimeState {
    Ready,
    FirstRun,
    IndexingInitial,
    IndexingBackground,
    Permission,
    Stale,
}


/// Decompress an embedded font at startup. Fonts ship zstd-compressed:
pub(super) fn show_app(app: &mut NeutraApp, ui: &mut Ui) {
    let more_events = app.process_events();
    if more_events {
        ui.ctx().request_repaint();
    } else if app.scanning || app.building_cache || app.tree_building || app.rank_pending {
        ui.ctx().request_repaint_after(Duration::from_millis(100));
    } else {
        ui.ctx().request_repaint_after(Duration::from_secs(if app.remote_watcher_started { 1 } else { 2 }));
    }

    let focus_search = ui.input_mut(|input| {
        input.consume_shortcut(&egui::KeyboardShortcut::new(
            egui::Modifiers::CTRL,
            egui::Key::K,
        ))
    });
    if focus_search {
        app.search_focus_requested = true;
    }
    let copy_selected = app.selected.is_some()
        && ui.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, egui::Key::Insert));
    if copy_selected {
        if let Some(path) = &app.selected {
            copy_to_clipboard(ui, path);
        }
    }
    let select_next =
        ui.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, egui::Key::ArrowDown));
    let select_previous =
        ui.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, egui::Key::ArrowUp));
    if select_next || select_previous {
        move_result_selection(app, select_next);
    }

    ui.spacing_mut().item_spacing = Vec2::ZERO;
    ui.set_min_size(ui.available_size());
    ui.painter().rect_filled(ui.max_rect(), 0.0, CANVAS);
    let state = runtime_state(app);
    ui.vertical(|ui| {
        fixed_strip(ui, MENU_H, BLACK, |ui| menu_bar(app, ui));
        if state == RuntimeState::FirstRun {
            let content_h = ui.available_height();
            ui.allocate_ui_with_layout(
                Vec2::new(ui.available_width(), content_h.max(0.0)),
                Layout::top_down(Align::LEFT),
                |ui| first_run_view(app, ui),
            );
            return;
        }

        fixed_strip(ui, QUERY_H, SURFACE, |ui| query_strip(app, ui));
        fixed_strip(ui, FILTER_H, SURFACE, |ui| kind_strip(app, ui));
        if matches!(
            state,
            RuntimeState::IndexingBackground | RuntimeState::Permission | RuntimeState::Stale
        ) {
            fixed_strip(ui, BANNER_H, banner_color(state), |ui| {
                runtime_banner(app, ui, state)
            });
        }

        let content_h = ui.available_height();
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), content_h.max(0.0)),
            Layout::top_down(Align::LEFT),
            |ui| {
                ui.set_min_size(Vec2::new(ui.available_width(), content_h.max(0.0)));
                match state {
                    RuntimeState::IndexingInitial => indexing_view(app, ui),
                    _ => ready_view(app, ui),
                }
            },
        );
    });
    // The modal survives only for first-run setup; everywhere else the
    // sidebar panel covers diagnostics.
    if state == RuntimeState::FirstRun {
        diagnostics_dialog(app, ui.ctx());
    }
    about_dialog(app, ui.ctx());
}

fn move_result_selection(app: &mut NeutraApp, forward: bool) {
    if app.hits.is_empty() {
        app.selected = None;
        return;
    }
    let current = app.selected.as_ref().and_then(|path| {
        app.hits
            .iter()
            .position(|hit| hit.record.path.as_ref() == path)
    });
    let position = match (current, forward) {
        (Some(position), true) => (position + 1).min(app.hits.len() - 1),
        (Some(position), false) => position.saturating_sub(1),
        (None, true) => 0,
        (None, false) => app.hits.len() - 1,
    };
    app.selected = Some(app.hits[position].record.path.to_string());
}

fn runtime_state(app: &NeutraApp) -> RuntimeState {
    if let Ok(forced) = std::env::var("NEUTRASEARCH_GUI_STATE") {
        match forced.to_ascii_lowercase().as_str() {
            "first-run" => return RuntimeState::FirstRun,
            "indexing" => {
                return if app.index_is_empty() {
                    RuntimeState::IndexingInitial
                } else {
                    RuntimeState::IndexingBackground
                }
            }
            "permission" => return RuntimeState::Permission,
            "stale" => return RuntimeState::Stale,
            _ => {}
        }
    }
    let has_results = !app.index_is_empty();
    // Only scanner/index problems justify the "retry as administrator"
    // banner. Action errors (failed open, settings write) stay in the
    // diagnostics list instead of masquerading as permission failures.
    let stale = app
        .lanes
        .iter()
        .any(|(key, lane)| lane.error && (key.contains("index") || key.contains("cache")));
    let has_scanner_error = app.lanes.iter().any(|(key, lane)| {
        lane.error
            && (key.contains("index")
                || key.contains("cache")
                || key == "helper"
                || key == "host"
                || key == "protocol"
                || key.contains("scan")
                || key.starts_with("remote:"))
    });
    if app.scanning || app.building_cache {
        if has_results {
            RuntimeState::IndexingBackground
        } else {
            RuntimeState::IndexingInitial
        }
    } else if stale && has_results {
        RuntimeState::Stale
    } else if !app.onboarding_complete {
        RuntimeState::FirstRun
    } else if has_scanner_error {
        RuntimeState::Permission
    } else {
        RuntimeState::Ready
    }
}


fn menu_bar(app: &mut NeutraApp, ui: &mut Ui) {
    bar_style(ui);
    ui.add_space(8.0);
    ui.add(egui::Image::new(&app.logo).fit_to_exact_size(Vec2::splat(20.0)));
    ui.add_space(6.0);
    ui.label(RichText::new(tracked("Neutrasearch")).font(sans(11.0)).strong());
    ui.add_space(12.0);

    if runtime_state(app) == RuntimeState::FirstRun {
        return;
    }

    ui.menu_button("File", |ui| {
        if ui.button("Locations and index").clicked() {
            app.diagnostics_open = true;
            app.sidebar_tab = SidebarTab::Locations;
            ui.close();
        }
        if ui.button("Rebuild index").clicked() {
            app.begin_scan();
            ui.close();
        }
        if ui
            .add_enabled(
                app.selected.is_some(),
                egui::Button::new("Copy selected path    Ctrl+Insert"),
            )
            .clicked()
        {
            if let Some(path) = &app.selected {
                copy_to_clipboard(ui, path);
            }
            ui.close();
        }
        ui.separator();
        if ui.button("Exit").clicked() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    });
    ui.menu_button("Search", |ui| {
        if ui.button("Focus search    Ctrl+K").clicked() {
            app.search_focus_requested = true;
            ui.close();
        }
        if ui.button("Clear search").clicked() {
            app.query.clear();
            app.requery();
            ui.close();
        }
        ui.separator();
        ui.menu_button(format!("Match    {}", app.search_mode.label()), |ui| {
            for mode in [SearchMode::Name, SearchMode::NameAndPath, SearchMode::Path] {
                if ui
                    .selectable_label(app.search_mode == mode, mode.label())
                    .clicked()
                {
                    app.search_mode = mode;
                    app.save_settings();
                    app.requery();
                    ui.close();
                }
            }
        });
        if app.selected_roots.len() > 1 {
            ui.menu_button("Location", |ui| {
                if ui
                    .selectable_label(app.scope_root.is_none(), "All selected folders")
                    .clicked()
                {
                    app.scope_root = None;
                    app.requery();
                    ui.close();
                }
                let roots = app
                    .selected_roots
                    .iter()
                    .map(|root| root.to_string_lossy().into_owned())
                    .collect::<Vec<_>>();
                for root in roots {
                    let selected = app.scope_root.as_deref() == Some(root.as_str());
                    if ui.selectable_label(selected, &root).clicked() {
                        app.scope_root = Some(root);
                        app.requery();
                        ui.close();
                    }
                }
            });
        }
        let case = ui
            .checkbox(&mut app.case_sensitive, "Match capitalisation (A \u{2260} a)")
            .changed();
        let words = ui.checkbox(&mut app.whole_word, "Whole words only").changed();
        let accents = ui
            .checkbox(&mut app.ignore_accents, "Ignore accents (caf\u{e9} = cafe)")
            .changed();
        let regex = ui
            .checkbox(&mut app.regex_mode, "Regular expression")
            .changed();
        if case || words || accents || regex {
            app.save_settings();
            app.requery();
        }
        ui.separator();
        ui.label(
            RichText::new("Ctrl+Up/Down selects results")
                .font(mono(8.5))
                .color(SUBTLE),
        );
    });
    ui.menu_button("View", |ui| {
        for view in ResultView::ALL {
            if ui
                .selectable_label(app.view_mode == view, view.label())
                .clicked()
            {
                app.view_mode = view;
                app.save_settings();
                ui.close();
            }
        }
    });
    ui.menu_button("Help", |ui| {
        if ui.button("About Neutrasearch").clicked() {
            app.about_open = true;
            ui.close();
        }
        ui.separator();
        ui.label(
            RichText::new("Ctrl+K Search · Ctrl+Up/Down Select · Ctrl+Insert Copy")
                .font(mono(8.5))
                .color(SUBTLE),
        );
        ui.separator();
        ui.hyperlink_to("Support on Ko-fi", "https://ko-fi.com/netroaki");
        ui.hyperlink_to("Support on Patreon", "https://www.patreon.com/NetroAki");
    });
}

fn query_strip(app: &mut NeutraApp, ui: &mut Ui) {
    ui.add_space(8.0);

    let before = app.query.clone();
    let can_search = !matches!(
        runtime_state(app),
        RuntimeState::FirstRun | RuntimeState::IndexingInitial
    );
    let response = egui::Frame::new()
        .fill(BLACK)
        .stroke(Stroke::new(1.0_f32, LINE_STRONG))
        .corner_radius(6)
        .inner_margin(Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                paint_search_icon(ui, MUTED);
                let response = ui.add_enabled(
                    can_search,
                    SearchField::new(&mut app.query)
                        .hint("Search everything by file name...")
                        .width((ui.available_width() - 81.0).max(160.0)),
                );
                egui::Frame::new()
                    .fill(SURFACE)
                    .stroke(Stroke::new(1.0_f32, LINE_STRONG))
                    .corner_radius(4)
                    .inner_margin(Margin::symmetric(6, 2))
                    .show(ui, |ui| {
                        ui.label(RichText::new("Ctrl + K").font(mono(CAPTION)).color(MUTED));
                    });
                response
            })
            .inner
        })
        .inner;
    if app.search_focus_requested {
        response.request_focus();
        app.search_focus_requested = false;
    }
    // The text edit drops focus on Esc, so losing focus on that frame means clear.
    if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Escape)) {
        // The changed-query check below issues the single requery.
        app.query.clear();
    }
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect.expand(2.0),
            6.0,
            Stroke::new(2.0_f32, GLOW.gamma_multiply(0.7)),
            StrokeKind::Outside,
        );
        // Enter/Down leave the search box for the results. The text edit consumes
        // arrow keys while focused, so this runs on the search response.
        let move_down = ui.input(|input| input.key_pressed(egui::Key::ArrowDown));
        let commit = ui.input(|input| input.key_pressed(egui::Key::Enter));
        if move_down || commit {
            if move_down {
                move_result_selection(app, true);
            }
            if let Some(path) = app.selected.clone() {
                surrender_widget_focus(ui);
                if commit {
                    perform_file_action(app, FileAction::Open(PathBuf::from(path)));
                }
            }
        }
    }
    if before != app.query {
        app.requery();
    }
    ui.add_space(8.0);
}

/// File-type presets under the search box: audio, images, video, programs,
/// compressed archives, documents, folders, or everything.
fn kind_strip(app: &mut NeutraApp, ui: &mut Ui) {
    ui.add_space(8.0);
    egui::ScrollArea::horizontal()
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                for preset in KindFilter::ALL {
                    if filter_tab(ui, preset.label(), app.kind_filter == preset).clicked()
                    {
                        app.kind_filter = preset;
                        app.save_settings();
                        app.requery();
                    }
                    ui.add_space(2.0);
                }
            });
        });
}

fn first_run_view(app: &mut NeutraApp, ui: &mut Ui) {
    ui.painter().rect_filled(ui.max_rect(), 0.0, CANVAS);
    let has_error = app.lanes.values().any(|lane| lane.error);
    ui.with_layout(Layout::top_down(Align::Center), |ui| {
        ui.add_space(54.0);
        ui.set_max_width(620.0);
        ui.label(
            RichText::new(if has_error {
                "Let's try that scan again"
            } else {
                "Choose where to search"
            })
            .font(sans(20.0))
            .strong(),
        );
        if has_error {
            ui.add_space(7.0);
            ui.label(
                RichText::new("Your folders are still selected and no existing index was changed.")
                    .font(sans(11.0))
                    .color(MUTED),
            );
        }
        ui.add_space(18.0);
        egui::Frame::new()
            .fill(SURFACE)
            .stroke(Stroke::new(
                1.0_f32,
                if has_error { ERROR } else { LINE_STRONG },
            ))
            .corner_radius(4)
            .inner_margin(Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(560.0_f32.min(ui.available_width()));
                if app.selected_roots.is_empty() {
                    ui.add_sized(
                        [ui.available_width(), 44.0],
                        egui::Label::new(
                            RichText::new("No folders selected")
                                .font(sans(11.0))
                                .color(SUBTLE),
                        ),
                    );
                } else {
                    let mut remove = None;
                    for (index, root) in app.selected_roots.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(root.display().to_string())
                                    .font(mono(10.0))
                                    .color(TEXT),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.small_button("Remove").clicked() {
                                    remove = Some(index);
                                }
                            });
                        });
                        if index + 1 < app.selected_roots.len() {
                            ui.separator();
                        }
                    }
                    if let Some(index) = remove {
                        app.remove_root(index);
                    }
                }
            });
        ui.add_space(12.0);
        ui.allocate_ui_with_layout(
            Vec2::new(560.0_f32.min(ui.available_width()), 36.0),
            Layout::left_to_right(Align::Center),
            |ui| {
                let action_width = if cfg!(target_os = "linux") {
                    252.0
                } else {
                    176.0
                };
                ui.add_space((ui.available_width() - action_width).max(0.0) * 0.5);
                let add = secondary_button(ui, "+  Add folder", ACID_STRONG);
                if app.setup_focus_requested && app.selected_roots.is_empty() {
                    add.request_focus();
                    app.setup_focus_requested = false;
                }
                if add.clicked() {
                    if let Some(folder) = rfd::FileDialog::new()
                        .set_title("Add search folder")
                        .pick_folder()
                    {
                        app.add_root(folder);
                    }
                }
                let scan_label = if has_error && cfg!(target_os = "linux") {
                    "Allow access and scan again"
                } else if has_error {
                    "Retry scan"
                } else if cfg!(target_os = "linux") {
                    "Allow access and scan"
                } else {
                    "Scan"
                };
                let scan = ui.add_enabled(
                    !app.selected_roots.is_empty(),
                    egui::Button::new(RichText::new(scan_label).font(sans(10.5)).strong())
                        .fill(ACID_STRONG)
                        .stroke(Stroke::new(1.0_f32, ACID_STRONG))
                        .corner_radius(2)
                        .min_size(Vec2::new(76.0, 32.0)),
                );
                if app.setup_focus_requested && !app.selected_roots.is_empty() {
                    scan.request_focus();
                    app.setup_focus_requested = false;
                }
                if scan.has_focus() {
                    ui.painter().rect_stroke(
                        scan.rect.expand(2.0),
                        3.0,
                        Stroke::new(1.0_f32, TEXT),
                        StrokeKind::Outside,
                    );
                }
                if scan.clicked() {
                    app.complete_onboarding_and_scan();
                }
                if has_error && ui.small_button("Review scan details").clicked() {
                    app.diagnostics_open = true;
                }
            },
        );
    });
}


fn indexing_view(app: &mut NeutraApp, ui: &mut Ui) {
    ui.painter().rect_filled(ui.max_rect(), 0.0, CANVAS);
    ui.add_space(34.0);
    ui.horizontal(|ui| {
        task_icon(ui, VIOLET);
        ui.add_space(10.0);
        ui.vertical(|ui| {
            ui.label(
                RichText::new("Building the first index")
                    .font(sans(18.0))
                    .strong(),
            );
            ui.label(
                RichText::new(
                    "Reachable selected locations are published together; unavailable locations are skipped.",
                )
                .font(sans(12.0))
                .color(MUTED),
            );
        });
    });
    ui.add_space(22.0);
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0_f32, LINE_STRONG))
        .inner_margin(Margin::same(15))
        .show(ui, |ui| {
            ui.label(
                RichText::new(format!("{} objects staged", fmt_count(app.scan_len())))
                    .font(sans(20.0))
                    .strong(),
            );
            ui.add_space(10.0);
            let (bar, _) = ui.allocate_exact_size(
                Vec2::new(ui.available_width().min(680.0), 6.0),
                Sense::hover(),
            );
            ui.painter().rect_filled(bar, 0.0, HOVER);
            let pulse =
                ((ui.ctx().input(|input| input.time) * 0.42).fract() as f32).clamp(0.0, 1.0);
            let segment = Rect::from_min_size(
                bar.left_top() + Vec2::new(bar.width() * pulse * 0.72, 0.0),
                Vec2::new(bar.width() * 0.28, bar.height()),
            );
            ui.painter().rect_filled(segment.intersect(bar), 0.0, VIOLET);
            ui.add_space(8.0);
            ui.label(
                RichText::new("Existing results remain untouched until the replacement is ready")
                .font(sans(10.5))
                .color(MUTED),
            );
        });
    ui.add_space(14.0);
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0_f32, LINE_STRONG))
        .inner_margin(Margin::same(15))
        .show(ui, |ui| {
            ui.label(RichText::new("Drives").font(sans(12.0)).strong());
            ui.add_space(6.0);
            let mut lanes: Vec<(&String, &LaneState)> = app
                .lanes
                .iter()
                .filter(|(key, _)| key.starts_with('/'))
                .collect();
            lanes.sort_by(|left, right| left.0.cmp(right.0));
            if lanes.is_empty() {
                ui.label(
                    RichText::new("Starting the scan…")
                        .font(sans(11.0))
                        .color(MUTED),
                );
            }
            for (path, lane) in lanes {
                let detail = if lane.records > 0 {
                    format!("{} objects · {} ms", fmt_count(lane.records), lane.ms)
                } else {
                    lane.status.clone()
                };
                ui.horizontal(|ui| {
                    ui.label(RichText::new(shorten(path, 48)).font(mono(10.0)).color(TEXT));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(shorten(&detail, 48))
                                .font(sans(10.5))
                                .color(if lane.error { ERROR } else { MUTED }),
                        );
                    });
                });
            }
        });
    ui.add_space(18.0);
    if secondary_button(ui, "Index details", MUTED).clicked() {
        app.diagnostics_open = true;
    }
}

fn ready_view(app: &mut NeutraApp, ui: &mut Ui) {
    if app.selected_roots.is_empty() {
        no_locations_view(app, ui);
        return;
    }
    fixed_strip(ui, TOOLBAR_H, SURFACE, |ui| results_toolbar(app, ui));
    let status_h = 26.0;
    let content_h = (ui.available_height() - status_h).max(0.0);
    ui.allocate_ui_with_layout(
        Vec2::new(ui.available_width(), content_h),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.set_min_height(content_h);
            let main_w = if app.diagnostics_open {
                (ui.available_width() - 472.0).max(200.0)
            } else {
                ui.available_width()
            };
            ui.allocate_ui_with_layout(
                Vec2::new(main_w, content_h),
                Layout::top_down(Align::LEFT),
                |ui| {
                    egui::Frame::new().fill(CANVAS).show(ui, |ui| match app.view_mode {
                        ResultView::Details => details_view(app, ui),
                        ResultView::List => list_view(app, ui),
                        ResultView::Grid => grid_view(app, ui),
                        ResultView::Treemap => treemap_view(app, ui),
                    });
                },
            );
            if app.diagnostics_open {
                ui.allocate_ui_with_layout(
                    Vec2::new(460.0, content_h.max(0.0)),
                    Layout::top_down(Align::LEFT),
                    |ui| {
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| sidebar::side_panel(app, ui));
                    },
                );
            }
        },
    );
    fixed_strip(ui, status_h, SURFACE, |ui| sidebar::status_bar(app, ui));
}

fn no_locations_view(app: &mut NeutraApp, ui: &mut Ui) {
    ui.centered_and_justified(|ui| {
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new("No folders selected")
                    .font(sans(18.0))
                    .strong(),
            );
            ui.add_space(6.0);
            ui.label(
                RichText::new("Add a folder to start searching again.")
                    .font(sans(11.0))
                    .color(MUTED),
            );
            ui.add_space(14.0);
            if primary_button(ui, "Add folder", ACID_STRONG).clicked() {
                if let Some(folder) = rfd::FileDialog::new()
                    .set_title("Add search folder")
                    .pick_folder()
                {
                    app.add_root(folder);
                    app.begin_scan_with_elevation(cfg!(target_os = "linux"));
                }
            }
        });
    });
}

fn results_toolbar(app: &mut NeutraApp, ui: &mut Ui) {
    ui.add_space(8.0);
    // The engine caps how many hits are materialized; report the full matched
    // count so a capped view never looks complete.
    let total = app.hits.len();
    let matched = app.search_stats.matched as usize;
    let label = if matched > total {
        format!(
            "{} of {} results",
            fmt_count(total as u64),
            fmt_count(matched as u64)
        )
    } else {
        format!("{} results", fmt_count(total as u64))
    };
    ui.label(RichText::new(label).font(sans(12.0)).strong());
    let (dot, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
    ui.painter().circle_filled(dot.center(), 3.0, if app.searching { WARN } else { icons::GREEN });
    ui.label(
        RichText::new(if app.searching {
            "Searching...".to_owned()
        } else {
            format!("Completed in {:.2}s", app.search_stats.wall_us as f64 / 1_000_000.0)
        })
        .font(sans(11.0))
        .color(MUTED),
    );
    if app.regex_mode && segment_button(ui, "Regex ×", true).clicked() {
        app.regex_mode = false;
        app.save_settings();
        app.requery();
    }
    if app.case_sensitive && segment_button(ui, "Aa ×", true).clicked() {
        app.case_sensitive = false;
        app.save_settings();
        app.requery();
    }
    if app.whole_word && segment_button(ui, "Words ×", true).clicked() {
        app.whole_word = false;
        app.save_settings();
        app.requery();
    }
    if app.ignore_accents && segment_button(ui, "Accents ×", true).clicked() {
        app.ignore_accents = false;
        app.save_settings();
        app.requery();
    }
    if app.search_mode != SearchMode::Name
        && segment_button(ui, &format!("{} ×", app.search_mode.label()), true).clicked()
    {
        app.search_mode = SearchMode::Name;
        app.save_settings();
        app.requery();
    }
    if app.scope_root.is_some() && segment_button(ui, "Folder scope ×", true).clicked() {
        app.scope_root = None;
        app.requery();
    }
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.add_space(8.0);
        ghost_style(ui);
        ui.menu_button(format!("{}  ▾", app.view_mode.label()), |ui| {
            for view in ResultView::ALL {
                if ui
                    .selectable_label(app.view_mode == view, view.label())
                    .clicked()
                {
                app.view_mode = view;
                app.save_settings();
                ui.close();
            }
        }
    });
        ui.spacing_mut().item_spacing.x = 0.0;
        if icons::view_button(ui, false, app.view_mode == ResultView::Grid).clicked() {
            app.view_mode = ResultView::Grid;
            app.save_settings();
        }
        if icons::view_button(ui, true, app.view_mode == ResultView::List).clicked() {
            app.view_mode = ResultView::List;
            app.save_settings();
        }
        ui.spacing_mut().item_spacing.x = 6.0;
        if let Some(path) = app.selected.clone() {
            ui.menu_button("Selected  ▾", |ui| {
                if ui.button("Open").clicked() {
                    perform_file_action(app, FileAction::Open(path.clone().into()));
                    ui.close();
                }
                if ui.button("Show in folder").clicked() {
                    perform_file_action(app, FileAction::Reveal(path.clone().into()));
                    ui.close();
                }
                if ui.button("Copy full path    Ctrl+Insert").clicked() {
                    copy_to_clipboard(ui, &path);
                    ui.close();
                }
            });
        }
    });
}



#[cfg(test)]
 mod tests {
    use super::*;
    use neutra_core::{Index, Query};
    use widgets::{civil_date, normalize_path};

    #[test]
    fn civil_dates_cover_epoch_and_recent_values() {
        assert_eq!(civil_date(0), (1970, 1, 1));
        assert_eq!(civil_date(20_454), (2026, 1, 1));
    }

    #[test]
    fn path_helpers_preserve_unix_windows_and_unc_hierarchies() {
        assert_eq!(
            normalize_path(r"/home/alex//Documents/"),
            "/home/alex/Documents"
        );
        assert_eq!(
            parent_path("/home/alex/Documents/file.txt"),
            "/home/alex/Documents"
        );
        assert_eq!(
            ancestor_paths("/home/alex"),
            vec!["/", "/home", "/home/alex"]
        );

        assert_eq!(normalize_path(r"C:\Users\Alex\"), "C:/Users/Alex");
        assert_eq!(parent_path(r"C:\Users\Alex"), "C:/Users");
        assert_eq!(
            ancestor_paths(r"C:\Users\Alex"),
            vec!["/", "C:/", "C:/Users", "C:/Users/Alex"]
        );
        assert_eq!(path_name("C:/"), "C:");

        assert_eq!(
            normalize_path(r"\\server\share\Folder\file.txt"),
            "//server/share/Folder/file.txt"
        );
        assert_eq!(parent_path("//server/share"), "/");
        assert_eq!(
            ancestor_paths("//server/share/Folder"),
            vec!["/", "//server/share", "//server/share/Folder"]
        );
    }

     /// Dense screenshot-style fixture for view tests. Formerly the GUI
     /// reference-mode dataset; reference mode itself is gone.
     fn dense_unicode_index() -> Index {
         fn record(
             path: String,
             size: u64,
             age_hours: i64,
             kind: FileKind,
             fs: neutra_core::FsKind,
             id: u64,
         ) -> FileRecord {
             let now = std::time::SystemTime::now()
                 .duration_since(std::time::UNIX_EPOCH)
                 .map(|duration| duration.as_secs() as i64)
                 .unwrap_or(0);
             FileRecord {
                 path: path.into(),
                 size,
                 mtime: now.saturating_sub(age_hours * 3_600),
                 mode: if kind == FileKind::Dir { 0o040755 } else { 0o100644 },
                 kind,
                 fs,
                 native_id: id,
                 native_parent: id.saturating_sub(1),
                 source: 0,
                 disk: size,
             }
         }
         let mut records = vec![
             record("/home/alex/Documents/Accounts".into(), 0, 3, FileKind::Dir, neutra_core::FsKind::Ext4, 10),
             record("/home/alex/Documents/Projects".into(), 0, 5, FileKind::Dir, neutra_core::FsKind::Ext4, 11),
             record("/home/alex/Documents".into(), 0, 4, FileKind::Dir, neutra_core::FsKind::Ext4, 12),
             record("/home/alex/Pictures".into(), 0, 30, FileKind::Dir, neutra_core::FsKind::Ext4, 13),
             record("/home/alex/Games".into(), 0, 40, FileKind::Dir, neutra_core::FsKind::Ext4, 14),
             record("/home/alex/Downloads/invoice-final.pdf".into(), 482 * 1024, 1, FileKind::File, neutra_core::FsKind::Ext4, 20),
             record("/home/alex/Documents/Accounts/invoice-tracker.xlsx".into(), 86 * 1024, 4, FileKind::File, neutra_core::FsKind::Ext4, 21),
             record("/home/alex/Documents/Accounts/invoice-template.docx".into(), 42 * 1024, 9, FileKind::File, neutra_core::FsKind::Ext4, 22),
             record("/home/alex/Documents/Accounts/发票-上海-七月.pdf".into(), 720 * 1024, 12, FileKind::File, neutra_core::FsKind::Ext4, 23),
             record("/home/alex/Documents/Accounts/فاتورة-يوليو.pdf".into(), 615 * 1024, 16, FileKind::File, neutra_core::FsKind::Ext4, 24),
             record("/home/alex/Documents/Accounts/चालान-जुलाई.pdf".into(), 530 * 1024, 20, FileKind::File, neutra_core::FsKind::Ext4, 25),
             record("/home/alex/Documents/Projects/Aurora/site-plan.svg".into(), 1_540 * 1024, 25, FileKind::File, neutra_core::FsKind::Ext4, 26),
             record("/home/alex/Pictures/Library/photo-library.bin".into(), 82_u64 << 30, 32, FileKind::File, neutra_core::FsKind::Ext4, 27),
             record("/home/alex/Games/Orion/game-data.pak".into(), 118_u64 << 30, 40, FileKind::File, neutra_core::FsKind::Ext4, 28),
             record("/home/alex/Videos/family-video.mp4".into(), 18_u64 << 30, 48, FileKind::File, neutra_core::FsKind::Ext4, 29),
             record("/var/lib/containers/container-storage.bin".into(), 39_u64 << 30, 52, FileKind::File, neutra_core::FsKind::Ext4, 30),
             record("/usr/lib/runtime-libraries.bin".into(), 27_u64 << 30, 55, FileKind::File, neutra_core::FsKind::Ext4, 31),
             record("/mnt/studio/Media/camera-originals.bin".into(), 164_u64 << 30, 60, FileKind::File, neutra_core::FsKind::Network("smb3".into()), 32),
         ];
         let folders = [
             "/home/alex/Documents/Accounts/2026",
             "/home/alex/Documents/Accounts/2025",
             "/mnt/studio/Clients/Invoices",
             "/home/alex/Downloads",
             "/home/alex/Work/Operations/Billing",
         ];
         let types = [
             ("pdf", 0_u64),
             ("pdf", 1),
             ("xlsx", 2),
             ("docx", 3),
             ("zip", 4),
             ("png", 5),
         ];
         for index in 0..48_u64 {
             let (extension, offset) = types[index as usize % types.len()];
             let magnitude = 180 * 1024 + ((index * 173 * 1024) % (9 * 1024 * 1024));
             records.push(record(
                 format!(
                     "{}/invoice-{}-{:02}-{:04}.{}",
                     folders[index as usize % folders.len()],
                     2026 - (index % 3),
                     index % 12 + 1,
                     index + 1042,
                     extension
                 ),
                 magnitude,
                 2 + index as i64 * 3,
                 FileKind::File,
                 if index % 5 == 2 {
                     neutra_core::FsKind::Network("smb3".into())
                 } else {
                     neutra_core::FsKind::Ext4
                 },
                 100 + index + offset,
             ));
         }
         let mut index = Index::new();
         index.extend(records);
         index
     }

     #[test]
     fn reference_index_exercises_dense_views_and_unicode_fallbacks() {
         let index = dense_unicode_index();
        assert!(index.len() >= 60);
        let mut query = Query::parse("");
        query.limit = 1_000;
        let (hits, _) = index.search(&query).unwrap();
        assert!(hits.iter().any(|hit| hit.record.path.contains("发票")));
        assert!(hits.iter().any(|hit| hit.record.path.contains("فاتورة")));
        assert!(hits.iter().any(|hit| hit.record.path.contains("चालान")));
    }
}
