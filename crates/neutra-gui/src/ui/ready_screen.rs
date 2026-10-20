use super::*;

pub(super) fn ready_view(app: &mut NeutraApp, ui: &mut Ui) {
    if app.selected_roots.is_empty() {
        no_locations_view(app, ui);
        return;
    }
    fixed_strip(ui, TOOLBAR_H, SURFACE, |ui| toolbar::show(app, ui));
    let status_h = 26.0;
    let content_h = (ui.available_height() - status_h).max(0.0);
    ui.allocate_ui_with_layout(
        Vec2::new(ui.available_width(), content_h),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.set_min_height(content_h);
            let main_w = if app.diagnostics_open {
                (ui.available_width() - 472.0).max(200.0)
            } else {
                ui.available_width()
            };
            ui.allocate_ui_with_layout(
                Vec2::new(main_w, content_h),
                Layout::top_down(Align::LEFT),
                |ui| {
                    egui::Frame::new()
                        .fill(CANVAS)
                        .show(ui, |ui| match app.view_mode {
                            ResultView::Details => details_view(app, ui),
                            ResultView::List => list_view(app, ui),
                            ResultView::Grid => grid_view(app, ui),
                            ResultView::Treemap => treemap_view(app, ui),
                        });
                },
            );
            if app.diagnostics_open {
                ui.allocate_ui_with_layout(
                    Vec2::new(460.0, content_h.max(0.0)),
                    Layout::top_down(Align::LEFT),
                    |ui| {
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| sidebar::side_panel(app, ui));
                    },
                );
            }
        },
    );
    fixed_strip(ui, status_h, SURFACE, |ui| sidebar::status_bar(app, ui));
}

pub(super) fn no_locations_view(app: &mut NeutraApp, ui: &mut Ui) {
    ui.centered_and_justified(|ui| {
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new("No folders selected")
                    .font(sans(18.0))
                    .strong(),
            );
            ui.add_space(6.0);
            ui.label(
                RichText::new("Add a folder to start searching again.")
                    .font(sans(11.0))
                    .color(MUTED),
            );
            ui.add_space(14.0);
            if primary_button(ui, "Add folder", ACID_STRONG).clicked() {
                if let Some(folder) = rfd::FileDialog::new()
                    .set_title("Add search folder")
                    .pick_folder()
                {
                    app.add_root(folder);
                    app.begin_scan_with_elevation(cfg!(target_os = "linux"));
                }
            }
        });
    });
}
