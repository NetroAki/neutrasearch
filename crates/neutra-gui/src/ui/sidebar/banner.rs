//! Runtime banner with dismiss. Dismissing hides it until a new error lane
//! or scan re-arms it.

use super::super::widgets::{
    BLUE, BLUE_DIM, ERROR, ERROR_DIM, MUTED, WARN, WARN_DIM, primary_button, sans,
    secondary_button,
};
use super::SidebarTab;
use crate::{LaneState, NeutraApp};
use egui::{Align, Color32, Layout, RichText, Stroke, Vec2};

type BannerCopy = (&'static str, String, Option<&'static str>, &'static str, Color32);

pub(crate) fn runtime_banner(app: &mut NeutraApp, ui: &mut egui::Ui, state: super::super::RuntimeState) {
    ui.add_space(10.0);
    let (marker, _) = ui.allocate_exact_size(Vec2::splat(24.0), egui::Sense::hover());
    let Some((title, detail, primary, secondary, color)) = banner_copy(app, state) else {
        return;
    };
    paint_marker(ui, &marker, color);
    ui.add_space(6.0);
    ui.vertical(|ui| {
        ui.label(RichText::new(title).font(sans(12.0)).strong());
        ui.label(RichText::new(detail).font(sans(10.5)).color(MUTED));
    });
    banner_actions(app, ui, state, primary, secondary, color);
}

fn banner_copy(app: &NeutraApp, state: super::super::RuntimeState) -> Option<BannerCopy> {
    match state {
        super::super::RuntimeState::IndexingBackground => Some((
            "Indexing in progress",
            "Existing results remain searchable.".to_owned(),
            None,
            "Index status",
            BLUE,
        )),
        super::super::RuntimeState::Permission => Some((
            "A native location is unavailable",
            permission_detail(app),
            primary_label(state),
            "Review folders and access",
            ERROR,
        )),
        super::super::RuntimeState::Stale => Some((
            "Results may be out of date",
            "The last complete index remains searchable.".to_owned(),
            primary_label(state),
            "Index status",
            WARN,
        )),
        _ => None,
    }
}

fn permission_detail(app: &NeutraApp) -> String {
    if app.index_is_empty() {
        "Review scanner access before building the first index.".to_owned()
    } else {
        "The last complete index remains searchable. Some locations could not be accessed.".to_owned()
    }
}

fn primary_label(state: super::super::RuntimeState) -> Option<&'static str> {
    match state {
        super::super::RuntimeState::Stale => Some("Rebuild now"),
        super::super::RuntimeState::Permission if cfg!(target_os = "windows") => {
            Some("Restart as Administrator")
        }
        super::super::RuntimeState::Permission if cfg!(target_os = "linux") => {
            Some("Retry as administrator")
        }
        super::super::RuntimeState::Permission => Some("Review access"),
        _ => None,
    }
}

pub(crate) fn banner_color(state: super::super::RuntimeState) -> Color32 {
    match state {
        super::super::RuntimeState::IndexingBackground => BLUE_DIM,
        super::super::RuntimeState::Permission => ERROR_DIM,
        super::super::RuntimeState::Stale => WARN_DIM,
        _ => super::super::widgets::SURFACE,
    }
}

fn paint_marker(ui: &mut egui::Ui, marker: &egui::Rect, color: Color32) {
    let dot = Stroke::new(1.5_f32, color);
    ui.painter().circle_stroke(marker.center(), 8.0, dot);
    ui.painter().line_segment(
        [marker.center() - Vec2::new(0.0, 3.0), marker.center() + Vec2::new(0.0, 2.0)],
        dot,
    );
    ui.painter().circle_filled(marker.center() + Vec2::new(0.0, 5.0), 1.0, color);
}

fn banner_actions(
    app: &mut NeutraApp,
    ui: &mut egui::Ui,
    state: super::super::RuntimeState,
    primary: Option<&str>,
    secondary: &str,
    color: Color32,
) {
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.add_space(8.0);
        dismiss_button(app, ui);
        if let Some(label) = primary {
            primary_action(app, ui, state, label, color);
        }
        if secondary_button(ui, secondary, MUTED).clicked() {
            app.diagnostics_open = true;
            app.sidebar_tab = tab_for(state);
        }
    });
}

fn dismiss_button(app: &mut NeutraApp, ui: &mut egui::Ui) {
    if ui.small_button("\u{d7}").clicked() {
        app.banner_hidden = true;
    }
}

fn primary_action(app: &mut NeutraApp, ui: &mut egui::Ui, state: super::super::RuntimeState, label: &str, color: Color32) {
    if !primary_button(ui, label, color).clicked() {
        return;
    }
    banner_primary(app, ui, state);
}

fn tab_for(state: super::super::RuntimeState) -> SidebarTab {
    if state == super::super::RuntimeState::Permission {
        SidebarTab::Locations
    } else {
        SidebarTab::Scanner
    }
}

fn banner_primary(app: &mut NeutraApp, ui: &mut egui::Ui, state: super::super::RuntimeState) {
    match state {
        super::super::RuntimeState::Stale => app.begin_scan(),
        super::super::RuntimeState::Permission if cfg!(target_os = "windows") => {
            windows_elevation(app, ui)
        }
        super::super::RuntimeState::Permission if cfg!(target_os = "linux") => {
            app.begin_scan_with_elevation(true)
        }
        super::super::RuntimeState::Permission => app.diagnostics_open = true,
        _ => {}
    }
}

fn windows_elevation(app: &mut NeutraApp, ui: &mut egui::Ui) {
    if let Err(error) = crate::request_elevated_restart() {
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
        return;
    }
    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
}
