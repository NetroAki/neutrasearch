//! Right-hand Locations-and-index panel plus status bar.

pub(crate) mod banner;
pub(crate) mod card;
pub(crate) mod sections;

use super::icons::tab_icon;
use super::widgets::{
    LINE_STRONG, MUTED, SUBTLE, SURFACE, TEXT, fmt_count, sans, segment_button,
};
use crate::NeutraApp;
use egui::{Align, Layout, Margin, RichText, Stroke, Vec2};

/// Sidebar tabs. Not persisted: the panel always reopens on Search Locations.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SidebarTab {
    #[default]
    Locations,
    Status,
    Scanner,
    Maintenance,
    Network,
}

impl SidebarTab {
    pub(super) const ALL: [Self; 5] = [
        Self::Locations,
        Self::Status,
        Self::Scanner,
        Self::Maintenance,
        Self::Network,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Locations => "Search Locations",
            Self::Status => "Index Status",
            Self::Scanner => "Scanner Details",
            Self::Maintenance => "Index Maintenance",
            Self::Network => "Network Folders",
        }
    }
}

pub(super) fn side_panel(app: &mut NeutraApp, ui: &mut egui::Ui) {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0_f32, LINE_STRONG))
        .corner_radius(4)
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            panel_header(app, ui);
            ui.add_space(8.0);
            panel_body(app, ui);
            if app.scanning || app.building_cache {
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                card::indexing_card(app, ui);
            }
        });
}

fn panel_header(app: &mut NeutraApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new("Locations and index").font(sans(14.0)).strong());
            ui.label(
                RichText::new("Configure where to search and manage the index.")
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

fn panel_body(app: &mut NeutraApp, ui: &mut egui::Ui) {
    // Explicit column widths: naked remainder layouts collapsed the tab
    // content to zero width here.
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            Vec2::new(124.0, 0.0),
            Layout::top_down(Align::LEFT),
            |ui| {
                for tab in SidebarTab::ALL {
                    nav_row(app, ui, tab);
                }
            },
        );
        ui.separator();
        let content_w = ui.available_width().max(50.0);
        ui.allocate_ui_with_layout(
            Vec2::new(content_w, 0.0),
            Layout::top_down(Align::LEFT),
            |ui| panel_tab(app, ui),
        );
    });
}

fn nav_row(app: &mut NeutraApp, ui: &mut egui::Ui, tab: SidebarTab) {
    let active = app.sidebar_tab == tab;
    ui.horizontal(|ui| {
        tab_icon(ui, tab, if active { TEXT } else { MUTED });
        if segment_button(ui, tab.label(), active).clicked() {
            app.sidebar_tab = tab;
        }
    });
}

fn panel_tab(app: &mut NeutraApp, ui: &mut egui::Ui) {
    // No inner scroll area: the ready-view wrapper already scrolls, and a
    // nested vertical ScrollArea inside the horizontal split measured zero
    // width for its content.
    match app.sidebar_tab {
        SidebarTab::Locations => sections::locations_section(app, ui),
        SidebarTab::Status => sections::status_section(app, ui),
        SidebarTab::Scanner => sections::scanner_section(app, ui),
        SidebarTab::Maintenance => sections::maintenance_section(app, ui),
        SidebarTab::Network => sections::network_section(app, ui),
    }
}

pub(super) fn status_bar(app: &NeutraApp, ui: &mut egui::Ui) {
    use super::icons::paint_db_icon;
    ui.horizontal(|ui| {
        paint_db_icon(ui, MUTED);
        let label = if app.index_is_empty() {
            "No index yet".to_owned()
        } else {
            format!("Index ready \u{b7} {} files", fmt_count(app.index_len()))
        };
        ui.label(RichText::new(label).font(sans(10.0)).color(MUTED));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(
                RichText::new(format!(
                    "Neutrasearch {} \u{b7} A Neutra Software project",
                    env!("CARGO_PKG_VERSION")
                ))
                .font(sans(10.0))
                .color(SUBTLE),
            );
        });
    });
}
