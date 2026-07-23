use super::*;
use egui::{
    Align, Align2, Color32, FontData, FontDefinitions, FontFamily, FontId, Id, Layout, Margin,
    Rect, RichText, Sense, Stroke, StrokeKind, TextStyle, Ui, Vec2,
};
use egui_expressive::widgets::SearchField;
use egui_expressive::{ResizableSplit, SplitAxis, Theme};
use regex::RegexBuilder;
use std::collections::BTreeMap;
use std::sync::Arc;

mod results;
mod treemap;

use results::{details_view, grid_view, list_view, perform_file_action, visible_indices};
use treemap::treemap_view;
pub(super) use treemap::Hierarchy;

const BLACK: Color32 = Color32::from_rgb(18, 19, 26);
const CANVAS: Color32 = Color32::from_rgb(23, 24, 33);
const SURFACE: Color32 = Color32::from_rgb(30, 31, 43);
const RAISED: Color32 = Color32::from_rgb(38, 40, 54);
const HOVER: Color32 = Color32::from_rgb(47, 49, 66);
const ACTIVE: Color32 = Color32::from_rgb(54, 56, 78);
const TEXT: Color32 = Color32::from_rgb(232, 233, 239);
const MUTED: Color32 = Color32::from_rgb(170, 172, 187);
const SUBTLE: Color32 = Color32::from_rgb(121, 124, 143);
const LINE: Color32 = Color32::from_rgb(47, 49, 63);
const LINE_STRONG: Color32 = Color32::from_rgb(67, 70, 89);
const ACID: Color32 = Color32::from_rgb(157, 162, 222);
const ACID_STRONG: Color32 = Color32::from_rgb(126, 132, 205);
const BLUE: Color32 = Color32::from_rgb(111, 145, 194);
const BLUE_DIM: Color32 = Color32::from_rgb(36, 46, 66);
const WARN: Color32 = Color32::from_rgb(224, 178, 74);
const WARN_DIM: Color32 = Color32::from_rgb(67, 53, 28);
const ERROR: Color32 = Color32::from_rgb(224, 101, 90);
const ERROR_DIM: Color32 = Color32::from_rgb(70, 35, 33);

const MENU_H: f32 = 30.0;
const QUERY_H: f32 = 44.0;
const TOOLBAR_H: f32 = 38.0;
const BANNER_H: f32 = 44.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ResultView {
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

pub(super) fn initial_view() -> ResultView {
    match std::env::var("NEUTRASEARCH_GUI_VIEW")
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "list" => ResultView::List,
        "grid" => ResultView::Grid,
        "treemap" => ResultView::Treemap,
        _ => ResultView::Details,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum KindFilter {
    All,
    Files,
    Folders,
}

impl KindFilter {
    const ALL: [Self; 3] = [Self::All, Self::Files, Self::Folders];

    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Files => "Files",
            Self::Folders => "Folders",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SortMode {
    Modified,
    Name,
    Size,
    Path,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SearchMode {
    Name,
    NameAndPath,
    Path,
}

impl SearchMode {
    fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::NameAndPath => "Name + path",
            Self::Path => "Path only",
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

pub(super) fn configure(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    register_font(
        &mut fonts,
        "neutra_sans",
        include_bytes!("../assets/fonts/NotoSans-Regular.ttf"),
    );
    register_font(
        &mut fonts,
        "neutra_mono",
        include_bytes!("../assets/fonts/NotoSansMono-Regular.ttf"),
    );
    register_font(
        &mut fonts,
        "neutra_arabic",
        include_bytes!("../assets/fonts/NotoSansArabic-Regular.ttf"),
    );
    register_font(
        &mut fonts,
        "neutra_devanagari",
        include_bytes!("../assets/fonts/NotoSansDevanagari-Regular.ttf"),
    );
    register_font(
        &mut fonts,
        "neutra_cjk",
        include_bytes!("../assets/fonts/NotoSansCJK-Regular.ttc"),
    );
    register_font(
        &mut fonts,
        "neutra_symbols",
        include_bytes!("../assets/fonts/NotoSansSymbols-Regular.ttf"),
    );
    register_font(
        &mut fonts,
        "neutra_symbols2",
        include_bytes!("../assets/fonts/NotoSansSymbols2-Regular.ttf"),
    );

    let proportional = vec![
        "neutra_sans",
        "neutra_arabic",
        "neutra_devanagari",
        "neutra_cjk",
        "neutra_symbols",
        "neutra_symbols2",
    ];
    let monospace = vec![
        "neutra_mono",
        "neutra_sans",
        "neutra_arabic",
        "neutra_devanagari",
        "neutra_cjk",
        "neutra_symbols",
        "neutra_symbols2",
    ];
    fonts.families.insert(
        FontFamily::Name("Neutra Sans".into()),
        proportional.iter().map(|name| (*name).to_owned()).collect(),
    );
    fonts.families.insert(
        FontFamily::Name("Neutra Mono".into()),
        monospace.iter().map(|name| (*name).to_owned()).collect(),
    );
    for name in proportional.into_iter().rev() {
        fonts
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, name.to_owned());
    }
    for name in monospace.into_iter().rev() {
        fonts
            .families
            .entry(FontFamily::Monospace)
            .or_default()
            .insert(0, name.to_owned());
    }
    ctx.set_fonts(fonts);

    Theme::dark().store(ctx);
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = CANVAS;
    visuals.window_fill = SURFACE;
    visuals.extreme_bg_color = BLACK;
    visuals.faint_bg_color = SURFACE;
    visuals.selection.bg_fill = ACTIVE;
    visuals.selection.stroke = Stroke::new(1.0_f32, ACID);
    visuals.widgets.noninteractive.bg_fill = SURFACE;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, LINE);
    visuals.widgets.noninteractive.corner_radius = 2.into();
    visuals.widgets.inactive.bg_fill = RAISED;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, LINE_STRONG);
    visuals.widgets.inactive.corner_radius = 2.into();
    visuals.widgets.hovered.bg_fill = HOVER;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, LINE_STRONG);
    visuals.widgets.hovered.corner_radius = 2.into();
    visuals.widgets.active.bg_fill = ACTIVE;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, ACID_STRONG);
    visuals.widgets.active.corner_radius = 2.into();
    visuals.widgets.open.bg_fill = HOVER;
    visuals.widgets.open.bg_stroke = Stroke::new(1.0_f32, ACID_STRONG);
    visuals.override_text_color = Some(TEXT);
    visuals.window_corner_radius = 3.into();
    visuals.menu_corner_radius = 2.into();
    visuals.popup_shadow = egui::epaint::Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha(150),
    };
    ctx.set_visuals(visuals);

    let sans_family = FontFamily::Name("Neutra Sans".into());
    let mono_family = FontFamily::Name("Neutra Mono".into());
    let mut style = (*ctx.global_style()).clone();
    style.spacing.item_spacing = Vec2::new(4.0, 3.0);
    style.spacing.button_padding = Vec2::new(8.0, 4.0);
    style.spacing.interact_size = Vec2::new(30.0, 30.0);
    style.spacing.menu_margin = Margin::same(5);
    style
        .text_styles
        .insert(TextStyle::Small, FontId::new(10.0, sans_family.clone()));
    style
        .text_styles
        .insert(TextStyle::Body, FontId::new(12.0, sans_family.clone()));
    style
        .text_styles
        .insert(TextStyle::Button, FontId::new(11.0, sans_family.clone()));
    style
        .text_styles
        .insert(TextStyle::Heading, FontId::new(18.0, sans_family));
    style
        .text_styles
        .insert(TextStyle::Monospace, FontId::new(11.0, mono_family));
    style.visuals = ctx.global_style().visuals.clone();
    ctx.set_global_style(style);
}

fn register_font(fonts: &mut FontDefinitions, name: &str, bytes: &'static [u8]) {
    fonts
        .font_data
        .insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
}

fn sans(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("Neutra Sans".into()))
}

fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("Neutra Mono".into()))
}

pub(super) fn show_app(app: &mut NeutraApp, ui: &mut Ui) {
    let more_events = app.process_events();
    if more_events {
        ui.ctx().request_repaint();
    } else if app.scanning || app.building_cache || app.tree_building {
        ui.ctx().request_repaint_after(Duration::from_millis(100));
    } else if app.remote_watcher_started {
        ui.ctx().request_repaint_after(Duration::from_secs(1));
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
    diagnostics_dialog(app, ui.ctx());
    about_dialog(app, ui.ctx());
}

fn copy_to_clipboard(ui: &Ui, text: &str) {
    ui.ctx().copy_text(text.to_owned());
    if let Ok(mut clipboard) = arboard::Clipboard::new() {
        let _ = clipboard.set_text(text.to_owned());
    }
}

fn fixed_strip(ui: &mut Ui, height: f32, fill: Color32, add: impl FnOnce(&mut Ui)) {
    let width = ui.available_width();
    ui.allocate_ui_with_layout(
        Vec2::new(width, height),
        Layout::left_to_right(Align::Center),
        |ui| {
            let rect = ui.max_rect();
            ui.painter().rect_filled(rect, 0.0, fill);
            ui.painter()
                .hline(rect.x_range(), rect.bottom(), Stroke::new(1.0_f32, LINE));
            add(ui);
        },
    );
}

fn move_result_selection(app: &mut NeutraApp, forward: bool) {
    let indices = visible_indices(app);
    if indices.is_empty() {
        app.selected = None;
        return;
    }
    let current = app.selected.as_ref().and_then(|path| {
        indices
            .iter()
            .position(|index| app.hits[*index].record.path.as_ref() == path)
    });
    let position = match (current, forward) {
        (Some(position), true) => (position + 1).min(indices.len() - 1),
        (Some(position), false) => position.saturating_sub(1),
        (None, true) => 0,
        (None, false) => indices.len() - 1,
    };
    app.selected = Some(app.hits[indices[position]].record.path.to_string());
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
    let has_error = app.lanes.values().any(|lane| lane.error);
    let stale = app
        .lanes
        .iter()
        .any(|(key, lane)| lane.error && (key.contains("index") || key.contains("cache")));
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
    } else if has_error {
        RuntimeState::Permission
    } else {
        RuntimeState::Ready
    }
}

fn banner_color(state: RuntimeState) -> Color32 {
    match state {
        RuntimeState::IndexingBackground => BLUE_DIM,
        RuntimeState::Permission => ERROR_DIM,
        RuntimeState::Stale => WARN_DIM,
        _ => SURFACE,
    }
}

fn menu_bar(app: &mut NeutraApp, ui: &mut Ui) {
    ui.visuals_mut().widgets.inactive.bg_fill = BLACK;
    ui.visuals_mut().widgets.inactive.bg_stroke = Stroke::NONE;
    ui.visuals_mut().widgets.inactive.corner_radius = 0.into();
    ui.add_space(8.0);
    ui.add(egui::Image::new(&app.logo).fit_to_exact_size(Vec2::splat(20.0)));
    ui.add_space(6.0);
    ui.label(RichText::new("Neutrasearch").font(sans(12.0)).strong());
    ui.add_space(12.0);

    if runtime_state(app) == RuntimeState::FirstRun {
        return;
    }

    ui.menu_button("File", |ui| {
        if ui.button("Locations and index").clicked() {
            app.diagnostics_open = true;
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
            .checkbox(&mut app.case_sensitive, "Case sensitive")
            .changed();
        let regex = ui
            .checkbox(&mut app.regex_mode, "Regular expression")
            .changed();
        if case || regex {
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
    paint_search_icon(ui, MUTED);
    ui.add_space(5.0);

    let field_width = (ui.available_width() - 26.0).max(220.0);
    let before = app.query.clone();
    let can_search = !matches!(
        runtime_state(app),
        RuntimeState::FirstRun | RuntimeState::IndexingInitial
    );
    let response = egui::Frame::new()
        .fill(BLACK)
        .stroke(Stroke::new(1.0_f32, LINE_STRONG))
        .corner_radius(3)
        .inner_margin(Margin::symmetric(6, 1))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let response = ui.add_enabled(
                    can_search,
                    SearchField::new(&mut app.query)
                        .hint("Search files and folders...")
                        .width((field_width - 44.0).max(160.0)),
                );
                ui.label(RichText::new("Ctrl K").font(mono(8.0)).color(SUBTLE));
                response
            })
            .inner
        })
        .inner;
    if app.search_focus_requested {
        response.request_focus();
        app.search_focus_requested = false;
    }
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect.expand(2.0),
            3.0,
            Stroke::new(1.0_f32, ACID_STRONG),
            StrokeKind::Outside,
        );
    }
    if before != app.query {
        app.requery();
    }
    ui.add_space(8.0);
}

fn paint_search_icon(ui: &mut Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(19.0), Sense::hover());
    let center = rect.center() - Vec2::new(1.5, 1.5);
    ui.painter()
        .circle_stroke(center, 5.0, Stroke::new(1.5_f32, color));
    ui.painter().line_segment(
        [center + Vec2::new(3.7, 3.7), center + Vec2::new(7.0, 7.0)],
        Stroke::new(1.5_f32, color),
    );
}

fn runtime_banner(app: &mut NeutraApp, ui: &mut Ui, state: RuntimeState) {
    ui.add_space(10.0);
    let (marker, _) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::hover());
    let (title, detail, primary, secondary, color) = match state {
        RuntimeState::IndexingBackground => (
            "Indexing in progress",
            "Existing results remain searchable.",
            "Run in background",
            "Index status",
            BLUE,
        ),
        RuntimeState::Permission => (
            "A native location is unavailable",
            if app.index_is_empty() {
                "Review scanner access before building the first index."
            } else {
                "The last complete index remains searchable."
            },
            if cfg!(target_os = "windows") {
                "Restart as Administrator"
            } else if cfg!(target_os = "linux") {
                "Retry as administrator"
            } else {
                "Review access"
            },
            "Review folders and access",
            ERROR,
        ),
        RuntimeState::Stale => (
            "Results may be out of date",
            "The last complete index remains searchable.",
            "Rebuild now",
            "Index status",
            WARN,
        ),
        _ => return,
    };
    ui.painter()
        .circle_stroke(marker.center(), 8.0, Stroke::new(1.5_f32, color));
    ui.painter().line_segment(
        [
            marker.center() - Vec2::new(0.0, 3.0),
            marker.center() + Vec2::new(0.0, 2.0),
        ],
        Stroke::new(1.5_f32, color),
    );
    ui.painter()
        .circle_filled(marker.center() + Vec2::new(0.0, 5.0), 1.0, color);
    ui.add_space(6.0);
    ui.vertical(|ui| {
        ui.label(RichText::new(title).font(sans(12.0)).strong());
        ui.label(RichText::new(detail).font(sans(10.5)).color(MUTED));
    });
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.add_space(8.0);
        if primary_button(ui, primary, color).clicked() {
            match state {
                RuntimeState::Stale => app.begin_scan(),
                RuntimeState::Permission if cfg!(target_os = "windows") => {
                    match request_elevated_restart() {
                        Ok(()) => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
                        Err(error) => {
                            app.lanes.insert(
                                "elevation".into(),
                                LaneState {
                                    label: "WINDOWS ELEVATION".into(),
                                    status: error,
                                    error: true,
                                    ..Default::default()
                                },
                            );
                            app.diagnostics_open = true;
                        }
                    }
                }
                RuntimeState::Permission if cfg!(target_os = "linux") => {
                    app.begin_scan_with_elevation(true)
                }
                RuntimeState::Permission => app.diagnostics_open = true,
                _ => {}
            }
        }
        if secondary_button(ui, secondary, MUTED).clicked() {
            app.diagnostics_open = true;
        }
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

fn task_icon(ui: &mut Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(40.0), Sense::hover());
    ui.painter().rect_filled(
        rect,
        3.0,
        Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 28),
    );
    ui.painter()
        .rect_stroke(rect, 3.0, Stroke::new(1.0_f32, color), StrokeKind::Inside);
    let inner = rect.shrink(5.0);
    let folder = Rect::from_min_size(inner.min + Vec2::new(3.0, 8.0), Vec2::new(24.0, 17.0));
    ui.painter()
        .rect_stroke(folder, 1.0, Stroke::new(1.5_f32, color), StrokeKind::Inside);
    ui.painter().line_segment(
        [folder.left_top(), folder.left_top() + Vec2::new(9.0, -4.0)],
        Stroke::new(1.5_f32, color),
    );
}

fn indexing_view(app: &mut NeutraApp, ui: &mut Ui) {
    ui.painter().rect_filled(ui.max_rect(), 0.0, CANVAS);
    ui.add_space(34.0);
    ui.horizontal(|ui| {
        task_icon(ui, BLUE);
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
            ui.painter().rect_filled(segment.intersect(bar), 0.0, BLUE);
            ui.add_space(8.0);
            ui.label(
                RichText::new("Existing results remain untouched until the replacement is ready")
                    .font(sans(10.5))
                    .color(MUTED),
            );
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
    egui::Frame::new()
        .fill(CANVAS)
        .show(ui, |ui| match app.view_mode {
            ResultView::Details => details_view(app, ui),
            ResultView::List => list_view(app, ui),
            ResultView::Grid => grid_view(app, ui),
            ResultView::Treemap => treemap_view(app, ui),
        });
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
    let visible = visible_indices(app);
    ui.add_space(10.0);
    ui.label(
        RichText::new(format!("{} results", fmt_count(visible.len() as u64)))
            .font(sans(12.0))
            .strong(),
    );
    if app.regex_mode && segment_button(ui, "Regex ×", true).clicked() {
        app.regex_mode = false;
        app.requery();
    }
    if app.case_sensitive && segment_button(ui, "Case ×", true).clicked() {
        app.case_sensitive = false;
        app.requery();
    }
    if app.search_mode != SearchMode::NameAndPath
        && segment_button(ui, &format!("{} ×", app.search_mode.label()), true).clicked()
    {
        app.search_mode = SearchMode::NameAndPath;
        app.requery();
    }
    if app.scope_root.is_some() && segment_button(ui, "Folder scope ×", true).clicked() {
        app.scope_root = None;
        app.requery();
    }
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.add_space(7.0);
        ui.menu_button(format!("{}  ▾", app.view_mode.label()), |ui| {
            for view in ResultView::ALL {
                if ui
                    .selectable_label(app.view_mode == view, view.label())
                    .clicked()
                {
                    app.view_mode = view;
                    ui.close();
                }
            }
        });
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
        ui.add_space(5.0);
        ui.separator();
        for filter in KindFilter::ALL.into_iter().rev() {
            if segment_button(ui, filter.label(), app.kind_filter == filter).clicked() {
                app.kind_filter = filter;
                app.requery();
            }
        }
    });
}

fn segment_button(ui: &mut Ui, label: &str, active: bool) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).font(sans(10.0)).color(if active {
            ACID
        } else {
            MUTED
        }))
        .fill(if active { ACTIVE } else { SURFACE })
        .stroke(Stroke::new(
            1.0_f32,
            if active {
                LINE_STRONG
            } else {
                Color32::TRANSPARENT
            },
        ))
        .corner_radius(1)
        .min_size(Vec2::new(0.0, 25.0)),
    )
}

fn diagnostics_dialog(app: &mut NeutraApp, ctx: &egui::Context) {
    if !app.diagnostics_open {
        return;
    }
    let mut open = app.diagnostics_open;
    let has_error = app.lanes.values().any(|lane| lane.error);
    egui::Window::new("Locations and index")
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(540.0)
        .min_width(420.0)
        .frame(
            egui::Frame::window(&ctx.global_style())
                .corner_radius(3)
                .stroke(Stroke::new(1.0_f32, LINE_STRONG))
                .fill(SURFACE),
        )
        .show(ctx, |ui| {
            locations_editor(app, ui);
            ui.add_space(10.0);
            ui.separator();
            ui.add_space(5.0);

            egui::CollapsingHeader::new("Index status")
                .default_open(has_error)
                .show(ui, |ui| {
                    diagnostic_row(ui, "Indexed items", &fmt_count(app.index_len()), false);
                    diagnostic_row(
                        ui,
                        "Index generation",
                        &app.last_generation.to_string(),
                        false,
                    );
                    diagnostic_row(
                        ui,
                        "Saved index location",
                        &app.cache_path.display().to_string(),
                        false,
                    );
                });

            egui::CollapsingHeader::new("Scanner details")
                .default_open(has_error)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(220.0)
                        .show(ui, |ui| {
                            for lane in app.lanes.values() {
                                let value = if lane.records > 0 {
                                    format!(
                                        "{} objects · {} ms · {}",
                                        fmt_count(lane.records),
                                        lane.ms,
                                        lane.status
                                    )
                                } else {
                                    lane.status.clone()
                                };
                                diagnostic_row(ui, &lane.label, &value, lane.error);
                            }
                        });
                });

            egui::CollapsingHeader::new("Index maintenance").show(ui, |ui| {
                ui.label(
                    RichText::new("Rebuilding replaces the index only after a complete scan.")
                        .font(sans(10.0))
                        .color(MUTED),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if secondary_button(
                        ui,
                        if app.scanning {
                            "Indexing..."
                        } else {
                            "Rebuild index"
                        },
                        ACID_STRONG,
                    )
                    .clicked()
                    {
                        app.begin_scan();
                    }
                    if cfg!(target_os = "linux")
                        && !app.scanning
                        && secondary_button(ui, "Rebuild as administrator", ACID_STRONG).clicked()
                    {
                        app.begin_scan_with_elevation(true);
                    }
                });
            });

            egui::CollapsingHeader::new("Network folders").show(ui, |ui| {
                ui.label(
                    RichText::new("Look for Neutrasearch helpers on mounted network servers.")
                        .font(sans(10.0))
                        .color(MUTED),
                );
                ui.add_space(6.0);
                if app.remote_watcher_started {
                    ui.label(
                        RichText::new("Watching for network servers")
                            .font(sans(10.5))
                            .color(BLUE),
                    );
                } else if secondary_button(ui, "Watch network servers", MUTED).clicked() {
                    spawn_network_watcher(app.tx.clone());
                    app.remote_watcher_started = true;
                }
            });
        });
    app.diagnostics_open = open;
}

fn locations_editor(app: &mut NeutraApp, ui: &mut Ui) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("SEARCH LOCATIONS")
                .font(sans(10.0))
                .color(SUBTLE)
                .strong(),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui
                .add_enabled(
                    !app.scanning && !app.building_cache,
                    egui::Button::new("+ Add folder").small(),
                )
                .clicked()
            {
                if let Some(folder) = rfd::FileDialog::new()
                    .set_title("Add search folder")
                    .pick_folder()
                {
                    app.add_root(folder);
                }
            }
        });
    });
    ui.add_space(5.0);
    if app.selected_roots.is_empty() {
        diagnostic_row(ui, "Locations", "No folders selected", false);
        return;
    }
    let mut remove = None;
    for (index, root) in app.selected_roots.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.add_sized(
                [ui.available_width() - 58.0, 28.0],
                egui::Label::new(
                    RichText::new(shorten(&root.display().to_string(), 72))
                        .font(mono(9.0))
                        .color(TEXT),
                ),
            );
            if ui
                .add_enabled(
                    !app.scanning && !app.building_cache,
                    egui::Button::new("Remove").small(),
                )
                .clicked()
            {
                remove = Some(index);
            }
        });
    }
    if let Some(index) = remove {
        app.remove_root(index);
    }
}

fn diagnostic_row(ui: &mut Ui, key: &str, value: &str, error: bool) {
    let width = ui.available_width();
    let response = ui.allocate_ui_with_layout(
        Vec2::new(width, 34.0),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.add_space(3.0);
            ui.add(
                egui::Label::new(RichText::new(key).font(sans(10.0)).color(if error {
                    ERROR
                } else {
                    MUTED
                }))
                .selectable(true),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(3.0);
                ui.add(
                    egui::Label::new(
                        RichText::new(shorten(value, 72))
                            .font(mono(8.5))
                            .color(if error { ERROR } else { TEXT }),
                    )
                    .selectable(true),
                );
            });
        },
    );
    ui.painter().hline(
        response.response.rect.x_range(),
        response.response.rect.bottom(),
        Stroke::new(1.0_f32, LINE),
    );
}

fn about_dialog(app: &mut NeutraApp, ctx: &egui::Context) {
    if !app.about_open {
        return;
    }
    let mut open = app.about_open;
    egui::Window::new("About Neutrasearch")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(430.0)
        .frame(
            egui::Frame::window(&ctx.global_style())
                .corner_radius(3)
                .stroke(Stroke::new(1.0_f32, LINE_STRONG))
                .fill(SURFACE),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.add(egui::Image::new(&app.logo).fit_to_exact_size(Vec2::splat(30.0)));
                ui.vertical(|ui| {
                    ui.label(RichText::new("Neutrasearch").font(sans(18.0)).strong());
                    ui.label(
                        RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION")))
                            .font(mono(9.0))
                            .color(MUTED),
                    );
                });
            });
            ui.add_space(12.0);
            ui.label(
                RichText::new("Fast native-metadata filename search without directory walking.")
                    .font(sans(11.0)),
            );
            ui.label(
                RichText::new("Created by NetroAki. Released under the MIT License.")
                    .font(sans(10.0))
                    .color(MUTED),
            );
            ui.add_space(10.0);
            ui.hyperlink_to(
                "Source · github.com/NetroAki/neutrasearch",
                "https://github.com/NetroAki/neutrasearch",
            );
            ui.horizontal(|ui| {
                ui.hyperlink_to("Ko-fi", "https://ko-fi.com/netroaki");
                ui.label(RichText::new("·").color(SUBTLE));
                ui.hyperlink_to("Patreon", "https://www.patreon.com/NetroAki");
            });
        });
    app.about_open = open;
}

fn primary_button(ui: &mut Ui, label: &str, color: Color32) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).font(sans(10.5)).color(BLACK).strong())
            .fill(color)
            .stroke(Stroke::new(1.0_f32, color))
            .corner_radius(2)
            .min_size(Vec2::new(0.0, 32.0)),
    )
}

fn secondary_button(ui: &mut Ui, label: &str, color: Color32) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).font(sans(10.5)).color(TEXT))
            .fill(RAISED)
            .stroke(Stroke::new(
                1.0_f32,
                if color == MUTED { LINE_STRONG } else { color },
            ))
            .corner_radius(2)
            .min_size(Vec2::new(0.0, 32.0)),
    )
}

fn is_drive_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'/'
}

fn normalize_path(path: &str) -> String {
    let replaced = path.replace('\\', "/");
    let unc = replaced.starts_with("//");
    let mut normalized = replaced
        .split('/')
        .filter(|component| !component.is_empty())
        .collect::<Vec<_>>()
        .join("/");
    if unc {
        normalized.insert_str(0, "//");
    } else if replaced.starts_with('/') {
        normalized.insert(0, '/');
    }
    if is_drive_path(&replaced) && normalized.len() == 2 {
        normalized.push('/');
    }
    if normalized.is_empty() {
        "/".into()
    } else {
        normalized
    }
}

fn parent_path(path: &str) -> String {
    let normalized = normalize_path(path);
    if normalized == "/" || is_volume_root(&normalized) {
        return "/".into();
    }
    normalized.rsplit_once('/').map_or_else(
        || "/".into(),
        |(parent, _)| {
            if parent.is_empty() {
                "/".into()
            } else if parent.len() == 2 && parent.ends_with(':') {
                format!("{parent}/")
            } else {
                parent.into()
            }
        },
    )
}

fn is_volume_root(path: &str) -> bool {
    if is_drive_path(path) {
        return path.len() == 3;
    }
    if let Some(unc) = path.strip_prefix("//") {
        return unc.split('/').filter(|part| !part.is_empty()).count() == 2;
    }
    false
}

fn path_name(path: &str) -> String {
    let normalized = normalize_path(path);
    if normalized == "/" {
        return "Computer".into();
    }
    if is_volume_root(&normalized) {
        return normalized.trim_end_matches('/').to_owned();
    }
    normalized
        .rsplit('/')
        .next()
        .unwrap_or(&normalized)
        .to_owned()
}

fn ancestor_paths(path: &str) -> Vec<String> {
    let normalized = normalize_path(path);
    let mut out = vec!["/".to_owned()];
    if is_drive_path(&normalized) {
        let root = normalized[..3].to_owned();
        out.push(root.clone());
        let mut current = root;
        for component in normalized[3..]
            .split('/')
            .filter(|component| !component.is_empty())
        {
            current.push_str(component);
            out.push(current.clone());
            current.push('/');
        }
    } else if let Some(unc) = normalized.strip_prefix("//") {
        let mut components = unc.split('/').filter(|component| !component.is_empty());
        if let (Some(server), Some(share)) = (components.next(), components.next()) {
            let mut current = format!("//{server}/{share}");
            out.push(current.clone());
            for component in components {
                current.push('/');
                current.push_str(component);
                out.push(current.clone());
            }
        }
    } else {
        let mut current = String::new();
        for component in normalized
            .split('/')
            .filter(|component| !component.is_empty())
        {
            current.push('/');
            current.push_str(component);
            out.push(current.clone());
        }
    }
    out.dedup();
    out
}

fn type_badge(record: &neutra_core::FileRecord) -> String {
    if record.kind == FileKind::Dir {
        return "DIR".into();
    }
    let ext = record.extension();
    if ext.is_empty() {
        "FILE".into()
    } else {
        ext.chars().take(4).collect::<String>().to_ascii_uppercase()
    }
}

fn type_color(record: &neutra_core::FileRecord) -> Color32 {
    if record.kind == FileKind::Dir {
        return BLUE;
    }
    extension_color(record.extension())
}

fn extension_color(extension: &str) -> Color32 {
    match extension.to_ascii_lowercase().as_str() {
        "pdf" => ERROR,
        "xls" | "xlsx" | "ods" | "csv" => ACID,
        "doc" | "docx" | "txt" | "md" | "rtf" => BLUE,
        "zip" | "7z" | "rar" | "tar" | "gz" | "pak" | "iso" => WARN,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "mp4" | "mkv" => {
            Color32::from_rgb(190, 112, 203)
        }
        _ => Color32::from_rgb(128, 146, 153),
    }
}

fn format_mtime(timestamp: i64) -> String {
    if timestamp <= 0 {
        return "Unknown".into();
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(timestamp, |duration| duration.as_secs() as i64);
    let age = now.saturating_sub(timestamp);
    if age < 60 {
        "Just now".into()
    } else if age < 3_600 {
        format!("{} min ago", age / 60)
    } else if age < 86_400 {
        format!("{} hr ago", age / 3_600)
    } else if age < 604_800 {
        format!("{} days ago", age / 86_400)
    } else {
        let (year, month, day) = civil_date(timestamp.div_euclid(86_400));
        format!("{year:04}-{month:02}-{day:02}")
    }
}

fn civil_date(days_since_epoch: i64) -> (i64, i64, i64) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

fn fmt_count(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn format_size(bytes: u64) -> String {
    if bytes >= 1 << 40 {
        format!("{:.1} TB", bytes as f64 / (1u64 << 40) as f64)
    } else if bytes >= 1 << 30 {
        format!("{:.1} GB", bytes as f64 / (1u64 << 30) as f64)
    } else if bytes >= 1 << 20 {
        format!("{:.1} MB", bytes as f64 / (1u64 << 20) as f64)
    } else if bytes >= 1 << 10 {
        format!("{:.1} KB", bytes as f64 / (1u64 << 10) as f64)
    } else {
        format!("{bytes} B")
    }
}

fn shorten(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    let tail = value
        .chars()
        .rev()
        .take(max_chars.saturating_sub(1))
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("…{tail}")
}

pub(super) fn reference_index() -> Index {
    use neutra_core::{FileRecord, FsKind};

    fn record(
        path: String,
        size: u64,
        age_hours: i64,
        kind: FileKind,
        fs: FsKind,
        id: u64,
    ) -> FileRecord {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs() as i64);
        FileRecord {
            path: path.into_boxed_str(),
            size,
            mtime: now.saturating_sub(age_hours * 3_600),
            mode: if kind == FileKind::Dir {
                0o040755
            } else {
                0o100644
            },
            kind,
            fs,
            native_id: id,
            native_parent: id.saturating_sub(1),
            source: 0,
        }
    }

    let mut records = vec![
        record(
            "/home/alex/Documents/Accounts".into(),
            0,
            3,
            FileKind::Dir,
            FsKind::Ext4,
            10,
        ),
        record(
            "/home/alex/Documents/Projects".into(),
            0,
            5,
            FileKind::Dir,
            FsKind::Ext4,
            11,
        ),
        record(
            "/home/alex/Downloads".into(),
            0,
            6,
            FileKind::Dir,
            FsKind::Ext4,
            12,
        ),
        record(
            "/mnt/studio/Clients/Invoices".into(),
            0,
            7,
            FileKind::Dir,
            FsKind::Network("smb3".into()),
            13,
        ),
        record(
            "/home/alex/Documents/Accounts/Invoice - Acme - June.pdf".into(),
            482 * 1024,
            1,
            FileKind::File,
            FsKind::Ext4,
            20,
        ),
        record(
            "/home/alex/Documents/Accounts/invoice-tracker.xlsx".into(),
            86 * 1024,
            4,
            FileKind::File,
            FsKind::Ext4,
            21,
        ),
        record(
            "/home/alex/Documents/Accounts/invoice-template.docx".into(),
            42 * 1024,
            9,
            FileKind::File,
            FsKind::Ext4,
            22,
        ),
        record(
            "/home/alex/Documents/Accounts/发票-上海-七月.pdf".into(),
            720 * 1024,
            12,
            FileKind::File,
            FsKind::Ext4,
            23,
        ),
        record(
            "/home/alex/Documents/Accounts/فاتورة-يوليو.pdf".into(),
            615 * 1024,
            16,
            FileKind::File,
            FsKind::Ext4,
            24,
        ),
        record(
            "/home/alex/Documents/Accounts/चालान-जुलाई.pdf".into(),
            530 * 1024,
            20,
            FileKind::File,
            FsKind::Ext4,
            25,
        ),
        record(
            "/home/alex/Documents/Projects/Aurora/site-plan.svg".into(),
            1_540 * 1024,
            25,
            FileKind::File,
            FsKind::Ext4,
            26,
        ),
        record(
            "/home/alex/Pictures/Library/photo-library.bin".into(),
            82_u64 << 30,
            32,
            FileKind::File,
            FsKind::Ext4,
            27,
        ),
        record(
            "/home/alex/Games/Orion/game-data.pak".into(),
            118_u64 << 30,
            40,
            FileKind::File,
            FsKind::Ext4,
            28,
        ),
        record(
            "/home/alex/Videos/family-video.mp4".into(),
            18_u64 << 30,
            48,
            FileKind::File,
            FsKind::Ext4,
            29,
        ),
        record(
            "/var/lib/containers/container-storage.bin".into(),
            39_u64 << 30,
            52,
            FileKind::File,
            FsKind::Ext4,
            30,
        ),
        record(
            "/usr/lib/runtime-libraries.bin".into(),
            27_u64 << 30,
            55,
            FileKind::File,
            FsKind::Ext4,
            31,
        ),
        record(
            "/mnt/studio/Media/camera-originals.bin".into(),
            164_u64 << 30,
            60,
            FileKind::File,
            FsKind::Network("smb3".into()),
            32,
        ),
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
                FsKind::Network("smb3".into())
            } else {
                FsKind::Ext4
            },
            100 + index + offset,
        ));
    }
    let mut index = Index::new();
    index.extend(records);
    index
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn reference_index_exercises_dense_views_and_unicode_fallbacks() {
        let index = reference_index();
        assert!(index.len() >= 60);
        let mut query = Query::parse("");
        query.limit = 1_000;
        let (hits, _) = index.search(&query);
        assert!(hits.iter().any(|hit| hit.record.path.contains("发票")));
        assert!(hits.iter().any(|hit| hit.record.path.contains("فاتورة")));
        assert!(hits.iter().any(|hit| hit.record.path.contains("चालान")));
    }
}
