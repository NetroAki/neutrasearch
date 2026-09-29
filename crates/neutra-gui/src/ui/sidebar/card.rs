//! Indexing progress card: staged count, indeterminate bar, per-drive
//! states, cancel. No percentages: with native lanes the total is unknown
//! until a drive finishes.

use super::super::icons::{paint_db_icon, paint_drive_icon};
use super::super::widgets::{
    BLUE, ERROR, HOVER, MUTED, TEXT, fmt_count, mono, sans, secondary_button, shorten,
};
use super::SidebarTab;
use crate::{LaneState, NeutraApp};
use egui::{Align, Layout, RichText, Vec2};

pub(crate) fn indexing_card(app: &mut NeutraApp, ui: &mut egui::Ui) {
    card_header(app, ui);
    ui.add_space(8.0);
    pulse_bar(ui);
    ui.add_space(6.0);
    ui.label(
        RichText::new(format!("{} objects staged", fmt_count(app.scan_len())))
            .font(sans(15.0))
            .strong(),
    );
    ui.label(
        RichText::new("Existing results remain untouched until the replacement is ready.")
            .font(sans(10.0))
            .color(MUTED),
    );
    ui.add_space(8.0);
    ui.label(RichText::new("Drives").font(sans(12.0)).strong());
    ui.add_space(4.0);
    drive_rows(app, ui);
    ui.add_space(10.0);
    card_actions(app, ui);
}

fn card_header(app: &mut NeutraApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        paint_db_icon(ui, BLUE);
        ui.vertical(|ui| {
            ui.label(RichText::new("Indexing in progress").font(sans(12.0)).strong());
            ui.label(
                RichText::new(
                    "Reachable locations are published together; unavailable locations are skipped.",
                )
                .font(sans(10.0))
                .color(MUTED),
            );
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.small_button("\u{d7}").clicked() {
                app.diagnostics_open = false;
            }
        });
    });
}

fn pulse_bar(ui: &mut egui::Ui) {
    let (bar, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 6.0), egui::Sense::hover());
    ui.painter().rect_filled(bar, 0.0, HOVER);
    let pulse = ((ui.ctx().input(|input| input.time) * 0.42).fract() as f32).clamp(0.0, 1.0);
    let segment = egui::Rect::from_min_size(
        bar.left_top() + Vec2::new(bar.width() * pulse * 0.72, 0.0),
        Vec2::new(bar.width() * 0.28, bar.height()),
    );
    ui.painter().rect_filled(segment.intersect(bar), 0.0, BLUE);
}

fn drive_rows(app: &NeutraApp, ui: &mut egui::Ui) {
    let mut lanes: Vec<(&String, &LaneState)> =
        app.lanes.iter().filter(|(key, _)| key.starts_with('/')).collect();
    lanes.sort_by(|left, right| left.0.cmp(right.0));
    if lanes.is_empty() {
        ui.label(RichText::new("Starting the scan\u{2026}").font(sans(11.0)).color(MUTED));
    }
    for (path, lane) in lanes {
        drive_row(ui, path, lane);
    }
}

fn drive_row(ui: &mut egui::Ui, path: &str, lane: &LaneState) {
    let detail = if lane.records > 0 {
        format!("{} objects \u{b7} {} ms", fmt_count(lane.records), lane.ms)
    } else {
        lane.status.clone()
    };
    ui.horizontal(|ui| {
        paint_drive_icon(ui, MUTED);
        ui.label(RichText::new(shorten(path, 40)).font(mono(10.0)).color(TEXT));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(
                RichText::new(shorten(&detail, 40))
                    .font(sans(10.5))
                    .color(if lane.error { ERROR } else { MUTED }),
            );
        });
    });
}

fn card_actions(app: &mut NeutraApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        if secondary_button(ui, "Index details", MUTED).clicked() {
            app.sidebar_tab = SidebarTab::Status;
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            #[cfg(not(target_os = "windows"))]
            if app.cancelling {
                ui.add_enabled(false, egui::Button::new("Cancelling\u{2026}").small());
            } else if secondary_button(ui, "Cancel indexing", ERROR).clicked() {
                app.cancel_scan();
            }
        });
    });
}
