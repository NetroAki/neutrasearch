//! Right-hand Locations-and-index panel plus status bar.

pub(crate) mod banner;
pub(crate) mod card;
pub(crate) mod sections;

use super::widgets::{LINE_STRONG, MUTED, SURFACE, TEXT, fmt_count, sans, tracked};
use crate::NeutraApp;
use egui::{Align, Layout, Margin, RichText, Stroke};

/// Sidebar tabs. Not persisted: the panel always reopens on Search Locations.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SidebarTab {
    #[default]
    Locations,
    Index,
    Network,
    Maintenance,
}

impl SidebarTab {
    pub(super) const ALL: [Self; 4] = [
        Self::Locations,
        Self::Index,
        Self::Network,
        Self::Maintenance,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Locations => "Locations",
            Self::Index => "Index",
            Self::Network => "Network",
            Self::Maintenance => "Maintenance",
        }
    }
}

pub(super) fn side_panel(app: &mut NeutraApp, ui: &mut egui::Ui) {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0_f32, LINE_STRONG))
        .corner_radius(8)
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

/// Overline: tracked 10px uppercase in surface-400, used for section titles.
pub(super) fn overline(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(tracked(text)).font(sans(10.0)).color(MUTED).strong());
}

fn panel_header(app: &mut NeutraApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(tracked("Locations & Index")).font(sans(11.0)).color(TEXT).strong());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.small_button("\u{d7}").clicked() {
                app.diagnostics_open = false;
            }
        });
    });
}

fn panel_body(app: &mut NeutraApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        for tab in SidebarTab::ALL {
            if super::icons::filter_tab(ui, tab.label(), app.sidebar_tab == tab).clicked() {
                app.sidebar_tab = tab;
            }
            ui.add_space(6.0);
        }
    });
    ui.add_space(6.0);
    panel_tab(app, ui);
}

fn panel_tab(app: &mut NeutraApp, ui: &mut egui::Ui) {
    // No inner scroll area: the ready-view wrapper already scrolls, and a
    // nested vertical ScrollArea inside the horizontal split measured zero
    // width for its content.
    match app.sidebar_tab {
        SidebarTab::Locations => sections::locations_section(app, ui),
        SidebarTab::Index => {
            sections::status_section(app, ui);
            ui.add_space(8.0);
            sections::scanner_section(app, ui);
        }
        SidebarTab::Maintenance => sections::maintenance_section(app, ui),
        SidebarTab::Network => sections::network_section(app, ui),
    }
}

pub(super) fn status_bar(app: &NeutraApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(
            RichText::new(if app.index_is_empty() {
                "No index yet".to_owned()
            } else {
                format!("{} files indexed", fmt_count(app.index_len()))
            })
            .font(sans(11.0))
            .color(MUTED),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(8.0);
            ui.label(RichText::new("Neutra Software").font(sans(11.0)).color(MUTED));
            ui.add_space(16.0);
            status_dot(app, ui);
        });
    });
}

fn status_dot(app: &NeutraApp, ui: &mut egui::Ui) {
    use super::icons::GREEN;
    use super::widgets::VIOLET;
    let (text, color) = if app.scanning || app.building_cache {
        ("Indexing\u{2026}", VIOLET)
    } else if app.lanes.values().any(|lane| lane.error) {
        ("Some locations unavailable", super::widgets::ERROR)
    } else {
        ("Index up to date", GREEN)
    };
    ui.label(RichText::new(text).font(sans(11.0)).color(MUTED));
    let (dot, _) = ui.allocate_exact_size(egui::Vec2::splat(10.0), egui::Sense::hover());
    ui.painter().circle_filled(dot.center(), 3.0, color);
}
