use super::*;

pub(crate) fn show_app(app: &mut NeutraApp, ui: &mut Ui) {
    ui.ctx().data_mut(|data| {
        data.insert_temp(
            Id::new("selected-file-paths"),
            Arc::new(app.selection.paths()),
        )
    });
    script_fonts::ensure(ui.ctx(), &app.query);
    file_shortcuts::update(app, ui);
    let more_events = app.process_events();
    if more_events {
        ui.ctx().request_repaint();
    } else if app.scanning || app.building_cache || app.tree_building || app.rank_pending {
        ui.ctx().request_repaint_after(Duration::from_millis(100));
    } else if !app
        .index_events
        .as_ref()
        .is_some_and(|events| events.active())
    {
        ui.ctx().request_repaint_after(Duration::from_millis(500));
    }

    let focus_search = ui.input_mut(|input| {
        input.consume_shortcut(&egui::KeyboardShortcut::new(
            egui::Modifiers::CTRL,
            egui::Key::K,
        ))
    });
    if focus_search {
        app.search_focus_requested = true;
    }
    let copy_selected = app.selected.is_some()
        && ui.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, egui::Key::Insert));
    if copy_selected {
        if let Some(path) = &app.selected {
            copy_to_clipboard(ui, path);
        }
    }
    let select_next =
        ui.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, egui::Key::ArrowDown));
    let select_previous =
        ui.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, egui::Key::ArrowUp));
    if select_next || select_previous {
        move_result_selection(app, select_next);
    }

    ui.spacing_mut().item_spacing = Vec2::ZERO;
    ui.set_min_size(ui.available_size());
    ui.painter().rect_filled(ui.max_rect(), 0.0, CANVAS);
    let state = runtime_state(app);
    ui.vertical(|ui| {
        fixed_strip(ui, MENU_H, BLACK, |ui| chrome::menu_bar(app, ui));
        if state == RuntimeState::FirstRun {
            let content_h = ui.available_height();
            ui.allocate_ui_with_layout(
                Vec2::new(ui.available_width(), content_h.max(0.0)),
                Layout::top_down(Align::LEFT),
                |ui| first_run_view(app, ui),
            );
            return;
        }

        fixed_strip(ui, QUERY_H, SURFACE, |ui| chrome::query_strip(app, ui));
        fixed_strip(ui, FILTER_H, SURFACE, |ui| chrome::kind_strip(app, ui));
        if matches!(
            state,
            RuntimeState::IndexingBackground | RuntimeState::Permission | RuntimeState::Stale
        ) {
            fixed_strip(ui, BANNER_H, banner_color(state), |ui| {
                runtime_banner(app, ui, state)
            });
        }

        let content_h = ui.available_height();
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), content_h.max(0.0)),
            Layout::top_down(Align::LEFT),
            |ui| {
                ui.set_min_size(Vec2::new(ui.available_width(), content_h.max(0.0)));
                match state {
                    RuntimeState::IndexingInitial => indexing_view(app, ui),
                    _ => ready_view(app, ui),
                }
            },
        );
    });
    // The modal survives only for first-run setup; everywhere else the
    // sidebar panel covers diagnostics.
    if state == RuntimeState::FirstRun {
        diagnostics_dialog(app, ui.ctx());
    }
    about_dialog(app, ui.ctx());
    shortcut_settings::show(app, ui.ctx());
    window::resize(ui);
}
