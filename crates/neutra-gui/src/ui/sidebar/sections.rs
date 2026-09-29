//! Sidebar tab bodies, shared with the legacy first-run dialog.

use super::super::dialogs::diagnostic_row;
use super::super::widgets::{
    ACID_STRONG, BLUE, MUTED, TEXT, fmt_count, format_mtime, mono, sans, secondary_button,
    shorten,
};
use crate::NeutraApp;
use egui::{Align, Layout, RichText};

pub(crate) fn locations_section(app: &mut NeutraApp, ui: &mut egui::Ui) {
    ui.label(RichText::new("Search Locations").font(sans(12.0)).strong());
    ui.label(
        RichText::new("Folders to include in the index.")
            .font(sans(10.0))
            .color(MUTED),
    );
    ui.add_space(4.0);
    if ui
        .add_enabled(
            !app.scanning && !app.building_cache && !app.cancelling,
            egui::Button::new("+ Add folder").small(),
        )
        .clicked()
    {
        if let Some(folder) =
            rfd::FileDialog::new().set_title("Add search folder").pick_folder()
        {
            app.add_root(folder);
        }
    }
    ui.add_space(6.0);
    location_rows(app, ui);
}

pub(crate) fn status_section(app: &NeutraApp, ui: &mut egui::Ui) {
    diagnostic_row(ui, "Indexed items", &fmt_count(app.index_len()), false);
    let index_updated = std::fs::metadata(&app.cache_path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| format_mtime(duration.as_secs() as i64))
        .unwrap_or_else(|| "unknown".into());
    diagnostic_row(ui, "Index updated", &index_updated, false);
    diagnostic_row(ui, "Index generation", &app.last_generation.to_string(), false);
    diagnostic_row(
        ui,
        "Saved index location",
        &app.cache_path.display().to_string(),
        false,
    );
}

pub(crate) fn scanner_section(app: &NeutraApp, ui: &mut egui::Ui) {
    egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
        for lane in app.lanes.values() {
            let value = if lane.records > 0 {
                format!("{} objects \u{b7} {} ms \u{b7} {}", fmt_count(lane.records), lane.ms, lane.status)
            } else {
                lane.status.clone()
            };
            diagnostic_row(ui, &lane.label, &value, lane.error);
        }
    });
}

pub(crate) fn maintenance_section(app: &mut NeutraApp, ui: &mut egui::Ui) {
    ui.label(
        RichText::new("Rebuilding replaces the index only after a complete scan.")
            .font(sans(10.0))
            .color(MUTED),
    );
    ui.add_space(6.0);
    rebuild_row(app, ui);
}

fn rebuild_row(app: &mut NeutraApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        let rebuilding = app.scanning || app.cancelling;
        if secondary_button(ui, if rebuilding { "Indexing\u{2026}" } else { "Rebuild index" }, ACID_STRONG)
            .clicked()
        {
            app.begin_scan();
        }
        #[cfg(target_os = "linux")]
        if !rebuilding
            && secondary_button(ui, "Rebuild as administrator", ACID_STRONG).clicked()
        {
            app.begin_scan_with_elevation(true);
        }
    });
}

pub(crate) fn network_section(app: &mut NeutraApp, ui: &mut egui::Ui) {
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
        crate::transport::spawn_network_watcher(app.tx.clone());
        app.remote_watcher_started = true;
    }
}

/// Location rows shared by the dialog and the sidebar tab: path label plus
/// an overflow menu per folder.
pub(crate) fn location_rows(app: &mut NeutraApp, ui: &mut egui::Ui) {
    if app.selected_roots.is_empty() {
        diagnostic_row(ui, "Locations", "No folders selected", false);
        return;
    }
    let mut remove = None;
    for (index, root) in app.selected_roots.iter().enumerate() {
        location_row(app, ui, index, root, &mut remove);
    }
    if let Some(index) = remove {
        app.remove_root(index);
    }
}

fn location_row(
    app: &NeutraApp,
    ui: &mut egui::Ui,
    index: usize,
    root: &std::path::PathBuf,
    remove: &mut Option<usize>,
) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(shorten(&root.display().to_string(), 56))
                .font(mono(9.0))
                .color(TEXT),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let overflow = ui.add_enabled(
                !app.scanning && !app.building_cache && !app.cancelling,
                egui::Button::new("\u{22ef}").small(),
            );
            overflow.context_menu(|menu| {
                if menu.button("Remove folder").clicked() {
                    *remove = Some(index);
                    menu.close();
                }
            });
        });
    });
}
