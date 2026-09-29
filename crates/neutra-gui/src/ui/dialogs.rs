//! Status banners and modal dialogs: runtime banners, the locations/index
//! diagnostics window, and the about window.

use super::*;
use super::widgets::*;

pub(super) fn runtime_banner(app: &mut NeutraApp, ui: &mut Ui, state: RuntimeState) {
    ui.add_space(10.0);
    let (marker, _) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::hover());
    // `primary` is an in-banner action; None keeps the secondary affordance
    // only. A live indexing banner has no useful primary action.
    let (title, detail, primary, secondary, color): (&str, &str, Option<&str>, &str, Color32) =
        match state {
            RuntimeState::IndexingBackground => (
                "Indexing in progress",
                "Existing results remain searchable.",
                None,
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
                Some(if cfg!(target_os = "windows") {
                    "Restart as Administrator"
                } else if cfg!(target_os = "linux") {
                    "Retry as administrator"
                } else {
                    "Review access"
                }),
                "Review folders and access",
                ERROR,
            ),
            RuntimeState::Stale => (
                "Results may be out of date",
                "The last complete index remains searchable.",
                Some("Rebuild now"),
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
        if let Some(primary) = primary {
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
        }
        if secondary_button(ui, secondary, MUTED).clicked() {
            app.diagnostics_open = true;
        }
    });
}

pub(super) fn banner_color(state: RuntimeState) -> Color32 {
    match state {
        RuntimeState::IndexingBackground => BLUE_DIM,
        RuntimeState::Permission => ERROR_DIM,
        RuntimeState::Stale => WARN_DIM,
        _ => SURFACE,
    }
}

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
                    diagnostic_row(ui, "Indexed items", &fmt_count(app.index_len()), false);
                    // The index file's mtime is its publish time; reading it
                    // shows staleness even for indexes built by the CLI.
                    let index_updated = std::fs::metadata(&app.cache_path)
                        .and_then(|metadata| metadata.modified())
                        .ok()
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
    let mut remove = None;
    for (index, root) in app.selected_roots.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.add_sized(
                [ui.available_width() - 58.0, 28.0],
                egui::Label::new(
                    RichText::new(shorten(&root.display().to_string(), 56))
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

pub(super) fn diagnostic_row(ui: &mut Ui, key: &str, value: &str, error: bool) {
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
