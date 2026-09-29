//! Status banners and modal dialogs: runtime banners, the locations/index
//! diagnostics window, and the about window.

use super::*;
use super::widgets::*;

pub(super) fn diagnostics_dialog(app: &mut NeutraApp, ctx: &egui::Context) {
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
                    super::sidebar::sections::status_section(app, ui);
                });

            egui::CollapsingHeader::new("Scanner details")
                .default_open(has_error)
                .show(ui, |ui| {
                    super::sidebar::sections::scanner_section(app, ui);
                });

            egui::CollapsingHeader::new("Index maintenance").show(ui, |ui| {
                super::sidebar::sections::maintenance_section(app, ui);
            });

            egui::CollapsingHeader::new("Network folders").show(ui, |ui| {
                super::sidebar::sections::network_section(app, ui);
            });
        });
    app.diagnostics_open = open;
}

pub(super) fn locations_editor(app: &mut NeutraApp, ui: &mut Ui) {
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
    super::sidebar::sections::location_rows(app, ui);
}

pub(super) fn diagnostic_row(ui: &mut Ui, key: &str, value: &str, error: bool) {
    let width = ui.available_width();
    // Narrow columns (sidebar tabs) stack key above value; wide rows keep
    // the two-column layout instead of clipping the value.
    if width < 300.0 {
        ui.vertical(|ui| {
            ui.label(RichText::new(key).font(sans(9.0)).color(if error { ERROR } else { MUTED }));
            ui.label(
                RichText::new(shorten(value, 64))
                    .font(mono(8.5))
                    .color(if error { ERROR } else { TEXT }),
            );
        });
        ui.painter().hline(
            egui::Rangef::new(ui.min_rect().left(), ui.min_rect().right()),
            ui.min_rect().bottom(),
            Stroke::new(1.0_f32, LINE),
        );
        return;
    }
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
                        RichText::new(shorten(value, 48))
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

pub(super) fn about_dialog(app: &mut NeutraApp, ctx: &egui::Context) {
    if !app.about_open {
        return;
    }
    let mut open = app.about_open;
    egui::Window::new("About Neutrasearch")
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
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
