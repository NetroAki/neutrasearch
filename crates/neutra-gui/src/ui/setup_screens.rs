use super::*;

pub(super) fn first_run_view(app: &mut NeutraApp, ui: &mut Ui) {
    ui.painter().rect_filled(ui.max_rect(), 0.0, CANVAS);
    let has_error = app.lanes.values().any(|lane| lane.error);
    ui.with_layout(Layout::top_down(Align::Center), |ui| {
        ui.add_space(54.0);
        ui.set_max_width(620.0);
        ui.label(
            RichText::new(if has_error {
                "Let's try that scan again"
            } else {
                "Choose where to search"
            })
            .font(sans(20.0))
            .strong(),
        );
        if has_error {
            ui.add_space(7.0);
            ui.label(
                RichText::new("Your folders are still selected and no existing index was changed.")
                    .font(sans(11.0))
                    .color(MUTED),
            );
        }
        ui.add_space(18.0);
        first_run_roots_card(app, ui, has_error);
        ui.add_space(12.0);
        ui.allocate_ui_with_layout(
            Vec2::new(560.0_f32.min(ui.available_width()), 36.0),
            Layout::left_to_right(Align::Center),
            |ui| first_run_actions(app, ui, has_error),
        );
    });
}

fn first_run_roots_card(app: &mut NeutraApp, ui: &mut Ui, has_error: bool) {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(
            1.0_f32,
            if has_error { ERROR } else { LINE_STRONG },
        ))
        .corner_radius(4)
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(560.0_f32.min(ui.available_width()));
            if app.selected_roots.is_empty() {
                ui.add_sized(
                    [ui.available_width(), 44.0],
                    egui::Label::new(
                        RichText::new("No folders selected")
                            .font(sans(11.0))
                            .color(MUTED),
                    ),
                );
            } else {
                let mut remove = None;
                for (index, root) in app.selected_roots.iter().enumerate() {
                    if root_row(ui, root) {
                        remove = Some(index);
                    }
                    if index + 1 < app.selected_roots.len() {
                        ui.separator();
                    }
                }
                if let Some(index) = remove {
                    app.remove_root(index);
                }
            }
        });
}

pub(super) fn indexing_view(app: &mut NeutraApp, ui: &mut Ui) {
    ui.painter().rect_filled(ui.max_rect(), 0.0, CANVAS);
    ui.add_space(34.0);
    ui.horizontal(|ui| {
        task_icon(ui, VIOLET);
        ui.add_space(10.0);
        ui.vertical(|ui| {
            ui.label(
                RichText::new("Building the first index")
                    .font(sans(18.0))
                    .strong(),
            );
            ui.label(
                RichText::new(
                    "Reachable selected locations are published together; unavailable locations are skipped.",
                )
                .font(sans(12.0))
                .color(MUTED),
            );
        });
    });
    ui.add_space(22.0);
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0_f32, LINE_STRONG))
        .inner_margin(Margin::same(15))
        .show(ui, |ui| {
            ui.label(
                RichText::new(format!("{} objects staged", fmt_count(app.scan_len())))
                    .font(sans(20.0))
                    .strong(),
            );
            ui.add_space(10.0);
            let (bar, _) = ui.allocate_exact_size(
                Vec2::new(ui.available_width().min(680.0), 6.0),
                Sense::hover(),
            );
            ui.painter().rect_filled(bar, 0.0, HOVER);
            let pulse =
                ((ui.ctx().input(|input| input.time) * 0.42).fract() as f32).clamp(0.0, 1.0);
            let segment = Rect::from_min_size(
                bar.left_top() + Vec2::new(bar.width() * pulse * 0.72, 0.0),
                Vec2::new(bar.width() * 0.28, bar.height()),
            );
            ui.painter()
                .rect_filled(segment.intersect(bar), 0.0, VIOLET);
            ui.add_space(8.0);
            ui.label(
                RichText::new("Existing results remain untouched until the replacement is ready")
                    .font(sans(10.5))
                    .color(MUTED),
            );
        });
    ui.add_space(14.0);
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0_f32, LINE_STRONG))
        .inner_margin(Margin::same(15))
        .show(ui, |ui| {
            ui.label(RichText::new("Drives").font(sans(12.0)).strong());
            ui.add_space(6.0);
            let mut lanes: Vec<(&String, &LaneState)> = app
                .lanes
                .iter()
                .filter(|(key, _)| key.starts_with('/'))
                .collect();
            lanes.sort_by(|left, right| left.0.cmp(right.0));
            if lanes.is_empty() {
                ui.label(
                    RichText::new("Starting the scan…")
                        .font(sans(11.0))
                        .color(MUTED),
                );
            }
            for (path, lane) in lanes {
                let detail = if lane.records > 0 {
                    format!("{} objects · {} ms", fmt_count(lane.records), lane.ms)
                } else {
                    lane.status.clone()
                };
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(shorten(path, 48))
                            .font(mono(10.0))
                            .color(TEXT),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(shorten(&detail, 48))
                                .font(sans(10.5))
                                .color(if lane.error { ERROR } else { MUTED }),
                        );
                    });
                });
            }
        });
    ui.add_space(18.0);
    if secondary_button(ui, "Index details", MUTED).clicked() {
        app.diagnostics_open = true;
    }
}

fn first_run_actions(app: &mut NeutraApp, ui: &mut Ui, has_error: bool) {
    let action_width = if cfg!(target_os = "linux") {
        252.0
    } else {
        176.0
    };
    ui.add_space((ui.available_width() - action_width).max(0.0) * 0.5);
    let add = secondary_button(ui, "+  Add folder", ACID_STRONG);
    if app.setup_focus_requested && app.selected_roots.is_empty() {
        add.request_focus();
        app.setup_focus_requested = false;
    }
    if add.clicked() {
        if let Some(folder) = rfd::FileDialog::new()
            .set_title("Add search folder")
            .pick_folder()
        {
            app.add_root(folder);
        }
    }
    let scan_label = if has_error && cfg!(target_os = "linux") {
        "Allow access and scan again"
    } else if has_error {
        "Retry scan"
    } else if cfg!(target_os = "linux") {
        "Allow access and scan"
    } else {
        "Scan"
    };
    let scan = ui.add_enabled(
        !app.selected_roots.is_empty(),
        egui::Button::new(RichText::new(scan_label).font(sans(10.5)).strong())
            .fill(ACID_STRONG)
            .stroke(Stroke::new(1.0_f32, ACID_STRONG))
            .corner_radius(2)
            .min_size(Vec2::new(76.0, 32.0)),
    );
    if app.setup_focus_requested && !app.selected_roots.is_empty() {
        scan.request_focus();
        app.setup_focus_requested = false;
    }
    if scan.has_focus() {
        ui.painter().rect_stroke(
            scan.rect.expand(2.0),
            3.0,
            Stroke::new(1.0_f32, TEXT),
            StrokeKind::Outside,
        );
    }
    if scan.clicked() {
        app.complete_onboarding_and_scan();
    }
    if has_error && ui.small_button("Review scan details").clicked() {
        app.diagnostics_open = true;
    }
}

fn root_row(ui: &mut Ui, root: &std::path::Path) -> bool {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(root.display().to_string())
                .font(mono(10.0))
                .color(TEXT),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.small_button("Remove").clicked()
        })
        .inner
    })
    .inner
}
