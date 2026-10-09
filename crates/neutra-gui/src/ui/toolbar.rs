use super::*;

pub(super) fn show(app: &mut NeutraApp, ui: &mut Ui) {
    ui.add_space(8.0);
    ui.spacing_mut().item_spacing.x = 8.0;
    // The engine caps how many hits are materialized; report the full matched
    // count so a capped view never looks complete.
    if app.view_mode != ResultView::Treemap {
        pages::controls(app, ui);
    }
    if !app.query.trim().is_empty() {
        let label = format!(
            "{} of {} results",
            fmt_count(app.hits.len() as u64),
            fmt_count(app.search_stats.matched)
        );
        ui.label(RichText::new(label).font(sans(12.0)).strong());
    }
    if ui.available_width() > 540.0 {
        if app.regex_mode && segment_button(ui, "Regex ×", true).clicked() {
            app.regex_mode = false;
            app.save_settings();
            app.requery();
        }
        if app.case_sensitive && segment_button(ui, "Aa ×", true).clicked() {
            app.case_sensitive = false;
            app.save_settings();
            app.requery();
        }
        if app.whole_word && segment_button(ui, "Words ×", true).clicked() {
            app.whole_word = false;
            app.save_settings();
            app.requery();
        }
        if app.ignore_accents && segment_button(ui, "Accents ×", true).clicked() {
            app.ignore_accents = false;
            app.save_settings();
            app.requery();
        }
        if app.search_mode != SearchMode::Name
            && segment_button(ui, &format!("{} ×", app.search_mode.label()), true).clicked()
        {
            app.search_mode = SearchMode::Name;
            app.save_settings();
            app.requery();
        }
        if app.scope_root.is_some() && segment_button(ui, "Folder scope ×", true).clicked() {
            app.scope_root = None;
            app.requery();
        }
    }
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.add_space(8.0);
        ghost_style(ui);
        ui.menu_button(format!("{}  ▾", app.view_mode.label()), |ui| {
            for view in ResultView::ALL {
                if ui
                    .selectable_label(app.view_mode == view, view.label())
                    .clicked()
                {
                    app.view_mode = view;
                    app.save_settings();
                    ui.close();
                }
            }
        });
        if ui.available_width() > 170.0 {
            ui.spacing_mut().item_spacing.x = 0.0;
            if icons::view_button(ui, false, app.view_mode == ResultView::Grid).clicked() {
                app.view_mode = ResultView::Grid;
                app.save_settings();
            }
            if icons::view_button(ui, true, app.view_mode == ResultView::List).clicked() {
                app.view_mode = ResultView::List;
                app.save_settings();
            }
        }
        ui.spacing_mut().item_spacing.x = 6.0;
        if let Some(path) = app.selected.clone() {
            ui.menu_button("Selected  ▾", |ui| {
                let mut action = None;
                file_actions::context_menu(ui, &path, &mut action);
                if let Some(action) = action {
                    app.file_operations.dispatch(action);
                }
            });
        }
    });
}
