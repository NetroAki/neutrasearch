//! Indexing progress card: staged count with a real percentage whenever
//! the previous scan left per-mount totals (native lanes never know the
//! total upfront), per-location details, cancel state.

use super::super::icons::{paint_db_icon, paint_drive_icon};
use super::super::widgets::{
    ACID_STRONG, VIOLET, ERROR, HOVER, LINE_STRONG, MUTED, TEXT, fmt_count, mono, sans,
    secondary_button, shorten,
};
use super::super::icons::GREEN;
use super::SidebarTab;
use crate::{LaneState, NeutraApp};
use egui::{Align, Color32, Layout, RichText, Stroke, Vec2};

pub(crate) fn indexing_card(app: &mut NeutraApp, ui: &mut egui::Ui) {
    card_header(app, ui);
    ui.add_space(8.0);
    let staged: u64 = app.staged_by_mount.values().sum();
    let overall = overall_fraction(app);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("{} objects staged", fmt_count(staged)))
                .font(sans(15.0))
                .strong(),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if let Some(fraction) = overall {
                ui.label(
                    RichText::new(format!("{}%", (fraction * 100.0).round() as u64))
                        .font(sans(15.0))
                        .strong(),
                );
            }
        });
    });
    ui.add_space(4.0);
    progress_bar(ui, overall);
    ui.add_space(6.0);
    ui.label(
        RichText::new("Reachable locations are published together; unavailable locations are skipped.")
            .font(sans(11.0))
            .color(MUTED),
    );
    ui.add_space(8.0);
    super::overline(ui, "Per-location details");
    ui.add_space(4.0);
    egui::Frame::new()
        .fill(super::super::widgets::RAISED)
        .stroke(Stroke::new(1.0_f32, LINE_STRONG))
        .corner_radius(8)
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            for (path, lane) in mount_lanes(app) {
                detail_row(app, ui, path, lane);
            }
        });
    ui.add_space(10.0);
    if secondary_button(ui, "View index details", MUTED).clicked() {
        app.sidebar_tab = SidebarTab::Index;
    }
}

fn card_header(app: &mut NeutraApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        paint_db_icon(ui, VIOLET);
        ui.vertical(|ui| {
            ui.label(RichText::new("Indexing in progress").font(sans(12.0)).strong());
            ui.label(
                RichText::new(
                    "Reachable locations are published together; unavailable locations are skipped.",
                )
                .font(sans(11.0))
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

/// Overall fraction when every active mount has a previous total; staged
/// counts above a stale total clamp instead of overshooting.
fn overall_fraction(app: &NeutraApp) -> Option<f32> {
    let mut staged = 0u64;
    let mut total = 0u64;
    for (path, _) in mount_lanes(app) {
        match app.mount_totals.get(path) {
            Some(known) if *known > 0 => {
                staged += app.staged_by_mount.get(path).copied().unwrap_or(0).min(*known);
                total += known;
            }
            _ => return None,
        }
    }
    if total == 0 {
        return None;
    }
    Some((staged as f64 / total as f64).min(1.0) as f32)
}

fn drive_fraction(app: &NeutraApp, path: &str) -> Option<f32> {
    let total = app.mount_totals.get(path).copied().filter(|total| *total > 0)?;
    let staged = app.staged_by_mount.get(path).copied().unwrap_or(0).min(total);
    Some((staged as f64 / total as f64).min(1.0) as f32)
}

fn progress_bar(ui: &mut egui::Ui, overall: Option<f32>) {
    let (bar, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 6.0), egui::Sense::hover());
    ui.painter().rect_filled(bar, 3.0, HOVER);
    match overall {
        Some(fraction) => {
            let fill = egui::Rect::from_min_size(bar.left_top(), Vec2::new(bar.width() * fraction, bar.height()));
            ui.painter().rect_filled(fill, 3.0, ACID_STRONG);
        }
        None => {
            let pulse = ((ui.ctx().input(|input| input.time) * 0.42).fract() as f32).clamp(0.0, 1.0);
            let segment = egui::Rect::from_min_size(
                bar.left_top() + Vec2::new(bar.width() * pulse * 0.72, 0.0),
                Vec2::new(bar.width() * 0.28, bar.height()),
            );
            ui.painter().rect_filled(segment.intersect(bar), 3.0, ACID_STRONG);
        }
    }
}

fn mount_lanes(app: &NeutraApp) -> Vec<(&String, &LaneState)> {
    let mut lanes: Vec<(&String, &LaneState)> =
        app.lanes.iter().filter(|(key, _)| key.starts_with('/')).collect();
    lanes.sort_by(|left, right| left.0.cmp(right.0));
    lanes
}

fn detail_row(app: &NeutraApp, ui: &mut egui::Ui, path: &str, lane: &LaneState) {
    let (status, color) = drive_state(app, path, lane);
    let right = drive_right(app, path, lane);
    ui.horizontal(|ui| {
        paint_drive_icon(ui, MUTED);
        ui.label(RichText::new(shorten(path, 34)).font(mono(10.0)).color(TEXT));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(right).font(sans(11.0)).color(MUTED));
            ui.label(RichText::new(status).font(sans(11.0)).color(color));
        });
    });
}

fn drive_state(app: &NeutraApp, path: &str, lane: &LaneState) -> (&'static str, Color32) {
    if lane.error {
        return ("Unavailable", ERROR);
    }
    if lane.records > 0 {
        return ("Ready", GREEN);
    }
    if drive_fraction(app, path).is_some() {
        ("Indexing", VIOLET)
    } else {
        ("Indexing\u{2026}", VIOLET)
    }
}

fn drive_right(app: &NeutraApp, path: &str, lane: &LaneState) -> String {
    if lane.error {
        return "\u{2014}".to_owned();
    }
    if lane.records > 0 {
        return format!("{} objects \u{b7} {}", fmt_count(lane.records), fmt_ms(lane.ms));
    }
    match app.staged_by_mount.get(path).copied().unwrap_or(0) {
        0 => "\u{2014}".to_owned(),
        staged => format!("{} objects staged", fmt_count(staged)),
    }
}

fn fmt_ms(ms: u64) -> String {
    if ms >= 1000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{ms} ms")
    }
}
