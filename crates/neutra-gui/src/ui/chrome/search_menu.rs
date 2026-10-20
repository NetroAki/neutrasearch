use super::*;

pub(super) fn show(app: &mut NeutraApp, ui: &mut Ui) {
    if ui.button("Keyboard shortcuts…").clicked() {
        if app.shortcuts.pending.is_none() {
            app.shortcuts = crate::shortcuts::ShortcutSettings::default();
        }
        app.shortcuts.open = true;
        ui.close();
    }
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
}
