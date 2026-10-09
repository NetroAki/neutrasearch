use super::scopes;
use super::*;

pub(super) fn menu_bar(app: &mut NeutraApp, ui: &mut Ui) {
    bar_style(ui);
    ui.add_space(8.0);
    ui.add(egui::Image::new(&app.logo).fit_to_exact_size(Vec2::splat(20.0)));
    ui.add_space(6.0);
    ui.label(
        RichText::new(tracked("Neutrasearch"))
            .font(sans(11.0))
            .strong(),
    );
    ui.add_space(12.0);

    if runtime_state(app) == RuntimeState::FirstRun {
        super::window::controls(ui);
        return;
    }

    ui.menu_button("File", |ui| {
        if ui.button("Locations and index").clicked() {
            app.diagnostics_open = true;
            app.sidebar_tab = SidebarTab::Locations;
            ui.close();
        }
        if ui.button("Rebuild index").clicked() {
            app.begin_scan();
            ui.close();
        }
        if ui
            .add_enabled(
                app.selected.is_some(),
                egui::Button::new("Copy selected path    Ctrl+Insert"),
            )
            .clicked()
        {
            if let Some(path) = &app.selected {
                copy_to_clipboard(ui, path);
            }
            ui.close();
        }
        ui.separator();
        if ui.button("Exit").clicked() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    });
    ui.menu_button("Search", |ui| {
        if ui.button("Focus search    Ctrl+K").clicked() {
            app.search_focus_requested = true;
            ui.close();
        }
        if ui.button("Clear search").clicked() {
            app.query.clear();
            app.requery();
            ui.close();
        }
        ui.separator();
        ui.menu_button(format!("Match    {}", app.search_mode.label()), |ui| {
            for mode in [SearchMode::Name, SearchMode::NameAndPath, SearchMode::Path] {
                if ui
                    .selectable_label(app.search_mode == mode, mode.label())
                    .clicked()
                {
                    app.search_mode = mode;
                    app.save_settings();
                    app.requery();
                    ui.close();
                }
            }
        });
        if app.selected_roots.len() > 1 {
            ui.menu_button("Location", |ui| {
                if ui
                    .selectable_label(app.scope_root.is_none(), "All selected folders")
                    .clicked()
                {
                    app.scope_root = None;
                    app.requery();
                    ui.close();
                }
                let roots = app
                    .selected_roots
                    .iter()
                    .map(|root| root.to_string_lossy().into_owned())
                    .collect::<Vec<_>>();
                for root in roots {
                    let selected = app.scope_root.as_deref() == Some(root.as_str());
                    if ui.selectable_label(selected, &root).clicked() {
                        app.scope_root = Some(root);
                        app.requery();
                        ui.close();
                    }
                }
            });
        }
        let case = ui
            .checkbox(
                &mut app.case_sensitive,
                "Match capitalisation (A \u{2260} a)",
            )
            .changed();
        let words = ui
            .checkbox(&mut app.whole_word, "Whole words only")
            .changed();
        let accents = ui
            .checkbox(&mut app.ignore_accents, "Ignore accents (caf\u{e9} = cafe)")
            .changed();
        let regex = ui
            .checkbox(&mut app.regex_mode, "Regular expression")
            .changed();
        if case || words || accents || regex {
            app.save_settings();
            app.requery();
        }
        ui.separator();
        ui.label(
            RichText::new("Ctrl+Up/Down selects results")
                .font(mono(8.5))
                .color(MUTED),
        );
    });
    ui.menu_button("View", |ui| {
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
    ui.menu_button("Help", |ui| {
        if ui.button("About Neutrasearch").clicked() {
            app.about_open = true;
            ui.close();
        }
        ui.separator();
        ui.label(
            RichText::new("Ctrl+K Search · Ctrl+Up/Down Select · Ctrl+Insert Copy")
                .font(mono(8.5))
                .color(MUTED),
        );
        ui.separator();
        ui.hyperlink_to("Support on Ko-fi", "https://ko-fi.com/netroaki");
        ui.hyperlink_to("Support on Patreon", "https://www.patreon.com/NetroAki");
    });
    super::window::controls(ui);
}

pub(super) fn query_strip(app: &mut NeutraApp, ui: &mut Ui) {
    ui.add_space(8.0);
    scopes::picker(app, ui);

    let before = app.query.clone();
    let can_search = !matches!(
        runtime_state(app),
        RuntimeState::FirstRun | RuntimeState::IndexingInitial
    );
    let response = egui::Frame::new()
        .fill(BLACK)
        .stroke(Stroke::new(1.0_f32, LINE_STRONG))
        .corner_radius(6)
        .inner_margin(Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                paint_search_icon(ui, MUTED);
                let response = ui.add_enabled(
                    can_search,
                    SearchField::new(&mut app.query)
                        .hint("Search everything by file name...")
                        .width((ui.available_width() - 81.0).max(160.0)),
                );
                egui::Frame::new()
                    .fill(SURFACE)
                    .stroke(Stroke::new(1.0_f32, LINE_STRONG))
                    .corner_radius(4)
                    .inner_margin(Margin::symmetric(6, 2))
                    .show(ui, |ui| {
                        ui.label(RichText::new("Ctrl + K").font(mono(CAPTION)).color(MUTED));
                    });
                response
            })
            .inner
        })
        .inner;
    if app.search_focus_requested {
        response.request_focus();
        app.search_focus_requested = false;
    }
    // The text edit drops focus on Esc, so losing focus on that frame means clear.
    if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Escape)) {
        // The changed-query check below issues the single requery.
        app.query.clear();
    }
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect.expand(2.0),
            6.0,
            Stroke::new(2.0_f32, ACID),
            StrokeKind::Outside,
        );
        // Enter/Down leave the search box for the results. The text edit consumes
        // arrow keys while focused, so this runs on the search response.
        let move_down = ui.input(|input| input.key_pressed(egui::Key::ArrowDown));
        let commit = ui.input(|input| input.key_pressed(egui::Key::Enter));
        if move_down || commit {
            if move_down {
                move_result_selection(app, true);
            }
            if let Some(path) = app.selected.clone() {
                surrender_widget_focus(ui);
                if commit {
                    perform_file_action(app, FileAction::Open(PathBuf::from(path)));
                }
            }
        }
    }
    if before != app.query {
        app.requery();
    }
    ui.add_space(8.0);
}

pub(super) fn kind_strip(app: &mut NeutraApp, ui: &mut Ui) {
    ui.add_space(8.0);
    egui::ScrollArea::horizontal().show(ui, |ui| {
        ui.horizontal(|ui| {
            for (index, preset) in KindFilter::ALL.into_iter().enumerate() {
                if index > 0 {
                    ui.separator();
                }
                if super::category_icons::button(ui, preset, app.kind_filter == preset).clicked() {
                    app.kind_filter = preset;
                    app.save_settings();
                    app.requery();
                }
                ui.add_space(2.0);
            }
        });
    });
}
