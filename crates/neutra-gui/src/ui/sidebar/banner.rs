//! Runtime banner with dismiss. Dismissing hides it until a new error lane
//! or scan re-arms it.

use super::super::widgets::{
    sans, secondary_button, ACID, MUTED, SELECTED, VIOLET, WARN, WARN_DIM,
};
use super::SidebarTab;
use crate::{LaneState, NeutraApp};
use egui::{Align, Color32, Layout, RichText, Stroke, Vec2};

type BannerCopy = (
    &'static str,
    String,
    Option<&'static str>,
    &'static str,
    Color32,
);

pub(crate) fn runtime_banner(
    app: &mut NeutraApp,
    ui: &mut egui::Ui,
    state: super::super::RuntimeState,
) {
    ui.add_space(10.0);
    let (marker, _) = ui.allocate_exact_size(Vec2::splat(24.0), egui::Sense::hover());
    let Some((title, detail, primary, secondary, color)) = banner_copy(app, state) else {
        return;
    };
    paint_marker(ui, &marker, color);
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).font(sans(12.0)).strong());
        ui.label(RichText::new("\u{b7}").font(sans(12.0)).color(MUTED));
        ui.label(RichText::new(detail).font(sans(11.0)).color(MUTED));
    });
    banner_actions(app, ui, state, primary, secondary);
}

fn banner_copy(app: &NeutraApp, state: super::super::RuntimeState) -> Option<BannerCopy> {
    match state {
        super::super::RuntimeState::IndexingBackground => Some((
            "Indexing in progress",
            "Existing results remain searchable.".to_owned(),
            None,
            "Index status",
            VIOLET,
        )),
        super::super::RuntimeState::Permission => Some((
            "Some folders couldn't be accessed",
            permission_detail(app),
            primary_label(state),
            "Review",
            WARN,
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
        "Previous index remains searchable.".to_owned()
    }
}

fn primary_label(state: super::super::RuntimeState) -> Option<&'static str> {
    match state {
        super::super::RuntimeState::Stale => Some("Rebuild now"),
        super::super::RuntimeState::Permission => Some("Try with elevated access"),
        _ => None,
    }
}

pub(crate) fn banner_color(state: super::super::RuntimeState) -> Color32 {
    match state {
        super::super::RuntimeState::IndexingBackground => SELECTED,
        super::super::RuntimeState::Permission => WARN_DIM,
        super::super::RuntimeState::Stale => WARN_DIM,
        _ => super::super::widgets::SURFACE,
    }
}

fn paint_marker(ui: &mut egui::Ui, marker: &egui::Rect, color: Color32) {
    let dot = Stroke::new(1.5_f32, color);
    ui.painter().circle_stroke(marker.center(), 8.0, dot);
    ui.painter().line_segment(
        [
            marker.center() - Vec2::new(0.0, 3.0),
            marker.center() + Vec2::new(0.0, 2.0),
        ],
        dot,
    );
    ui.painter()
        .circle_filled(marker.center() + Vec2::new(0.0, 5.0), 1.0, color);
}

fn banner_actions(
    app: &mut NeutraApp,
    ui: &mut egui::Ui,
    state: super::super::RuntimeState,
    primary: Option<&str>,
    secondary: &str,
) {
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.add_space(8.0);
        if let Some(label) = primary {
            if secondary_button(ui, label, MUTED).clicked() {
                banner_primary(app, ui, state);
            }
        }
        ui.label(RichText::new("|").font(sans(11.0)).color(MUTED));
        if ui
            .add(
                egui::Button::new(RichText::new(secondary).font(sans(11.0)).color(ACID))
                    .frame(false),
            )
            .clicked()
        {
            app.diagnostics_open = true;
            app.sidebar_tab = tab_for(state);
        }
    });
}

fn tab_for(state: super::super::RuntimeState) -> SidebarTab {
    if state == super::super::RuntimeState::Permission {
        SidebarTab::Locations
    } else {
        SidebarTab::Index
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
