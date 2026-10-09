//! Sidebar tab bodies, shared with the legacy first-run dialog.

use super::super::dialogs::diagnostic_row;
use super::super::widgets::{
    fmt_count, format_mtime, mono, sans, secondary_button, shorten, ACID_STRONG, MUTED, TEXT,
};
use crate::NeutraApp;
use egui::{Align, Layout, RichText};

pub(crate) fn locations_section(app: &mut NeutraApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        super::overline(ui, "Search locations");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui
                .add_enabled(
                    !app.scanning && !app.building_cache && !app.cancelling,
                    egui::Button::new("+ Add location").small(),
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
    ui.label(
        RichText::new("Folders to include in the index.")
            .font(sans(11.0))
            .color(MUTED),
    );
    ui.add_space(6.0);
    egui::Frame::new()
        .fill(super::super::widgets::BLACK)
        .stroke(egui::Stroke::new(1.0_f32, super::super::widgets::LINE))
        .corner_radius(8)
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| location_rows(app, ui));
}

pub(crate) fn status_section(app: &NeutraApp, ui: &mut egui::Ui) {
    diagnostic_row(ui, "Indexed items", &fmt_count(app.index_len()), false);
    let index_updated = [&app.cache_path, &app.cache_path.with_extension("delta")]
        .into_iter()
        .filter_map(|path| std::fs::metadata(path).ok()?.modified().ok())
        .max()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| format_mtime(duration.as_secs() as i64))
        .unwrap_or_else(|| "unknown".into());
    diagnostic_row(ui, "Index updated", &index_updated, false);
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
}

pub(crate) fn scanner_section(app: &NeutraApp, ui: &mut egui::Ui) {
    egui::ScrollArea::vertical()
        .max_height(220.0)
        .show(ui, |ui| {
            for lane in app.lanes.values() {
                let value = if lane.records > 0 {
                    format!(
                        "{} objects \u{b7} {} ms \u{b7} {}",
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
}

pub(crate) fn maintenance_section(app: &mut NeutraApp, ui: &mut egui::Ui) {
    ui.label(
        RichText::new("Rebuilding replaces the index only after a complete scan.")
            .font(sans(11.0))
            .color(MUTED),
    );
    ui.add_space(6.0);
    rebuild_row(app, ui);
}

fn rebuild_row(app: &mut NeutraApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        let rebuilding = app.scanning || app.cancelling;
        if secondary_button(
            ui,
            if rebuilding {
                "Indexing\u{2026}"
            } else {
                "Rebuild index"
            },
            ACID_STRONG,
        )
        .clicked()
        {
            app.begin_scan();
        }
        #[cfg(not(target_os = "windows"))]
        if app.scanning {
            if app.cancelling {
                ui.add_enabled(false, egui::Button::new("Cancelling\u{2026}").small());
            } else if secondary_button(ui, "Cancel running scan", super::super::widgets::ERROR)
                .clicked()
            {
                app.cancel_scan();
            }
        }
        #[cfg(target_os = "linux")]
        if !rebuilding && secondary_button(ui, "Rebuild as administrator", ACID_STRONG).clicked() {
            app.begin_scan_with_elevation(true);
        }
    });
}

pub(crate) fn network_section(app: &mut NeutraApp, ui: &mut egui::Ui) {
    ui.label(
        RichText::new("Look for Neutrasearch helpers on mounted network servers.")
            .font(sans(11.0))
            .color(MUTED),
    );
    ui.add_space(6.0);
    if app.remote_watcher_started {
        ui.label(
            RichText::new("Watching for network servers")
                .font(sans(11.0))
                .color(MUTED),
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
    for (index, root) in app.selected_roots.clone().iter().enumerate() {
        let root_text = root.display().to_string();
        let (status, color) = root_status(app, &root_text);
        ui.horizontal(|ui| {
            location_icon(ui, &root_text);
            let response = ui.add(
                egui::Label::new(
                    RichText::new(shorten(&root_text, 34))
                        .font(mono(11.0))
                        .color(TEXT),
                )
                .sense(egui::Sense::click()),
            );
            response.context_menu(|menu| {
                if menu.button("Search this location").clicked() {
                    app.scope_root = Some(root_text.clone());
                    app.requery();
                    menu.close();
                }
                if menu.button("Open in file manager").clicked() {
                    app.file_operations
                        .dispatch(crate::transport::file_actions::Action::Open(root.clone()));
                    menu.close();
                }
                if menu.button("Copy full path").clicked() {
                    menu.ctx().copy_text(root_text.clone());
                    menu.close();
                }
                if menu
                    .add_enabled(
                        !app.scanning && !app.building_cache,
                        egui::Button::new("Remove from indexed locations"),
                    )
                    .clicked()
                {
                    remove = Some(index);
                    menu.close();
                }
            });
            row_tail(app, ui, index, &root_text, status, color, &mut remove);
        });
    }
    if let Some(index) = remove {
        app.remove_root(index);
    }
}

fn location_icon(ui: &mut egui::Ui, root: &str) {
    if root == "/home" || root.starts_with("/home/") {
        super::super::icons::paint_home_icon(ui, MUTED);
    } else {
        super::super::icons::paint_drive_icon(ui, MUTED);
    }
}

fn row_tail(
    app: &NeutraApp,
    ui: &mut egui::Ui,
    index: usize,
    root_text: &str,
    status: &'static str,
    color: egui::Color32,
    remove: &mut Option<usize>,
) {
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        row_menu(app, ui, index, root_text, remove);
        ui.label(RichText::new(status).font(sans(11.0)).color(color));
        let (dot, _) = ui.allocate_exact_size(egui::Vec2::splat(10.0), egui::Sense::hover());
        ui.painter().circle_filled(dot.center(), 3.0, color);
    });
}

fn row_menu(
    app: &NeutraApp,
    ui: &mut egui::Ui,
    index: usize,
    root_text: &str,
    remove: &mut Option<usize>,
) {
    ui.add_enabled_ui(
        !app.scanning && !app.building_cache && !app.cancelling,
        |ui| {
            ui.menu_button("\u{22ef}", |menu| {
                if menu.button("Copy path").clicked() {
                    super::super::widgets::copy_to_clipboard(menu, root_text);
                    menu.close();
                }
                if menu.button("Remove folder").clicked() {
                    *remove = Some(index);
                    menu.close();
                }
            });
        },
    );
}

/// Aggregate lane state for one selected root: any failed lane reads
/// Unavailable, an unfinished scan reads Indexing, finished lanes Ready.
fn root_status(app: &NeutraApp, root: &str) -> (&'static str, egui::Color32) {
    use super::super::icons::GREEN;
    use super::super::widgets::{ERROR, MUTED, VIOLET};
    let failed = app
        .lanes
        .iter()
        .any(|(key, lane)| lane.error && covers(key, root));
    if failed {
        return ("Unavailable", ERROR);
    }
    let known: Vec<&crate::LaneState> = app
        .lanes
        .iter()
        .filter(|(key, _)| covers(key, root))
        .map(|(_, lane)| lane)
        .collect();
    if known.is_empty() || !app.scanning {
        if app.index_is_empty() {
            return ("-", MUTED);
        }
        return ("Ready", GREEN);
    }
    if known.iter().all(|lane| lane.records > 0) {
        ("Ready", GREEN)
    } else {
        ("Indexing\u{2026}", VIOLET)
    }
}

/// A lane covers a root when its mountpoint is the root or an ancestor.
fn covers(key: &str, root: &str) -> bool {
    key.starts_with('/') && (key == "/" || key == root || root.starts_with(&format!("{key}/")))
}
