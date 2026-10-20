use super::*;

pub(in crate::ui) fn show(app: &mut NeutraApp, ui: &mut Ui) {
    if app.searching || (app.search_seq == 0 && app.query.is_empty()) {
        return if app.searching {
            icons::searching_placeholder(ui)
        } else {
            icons::idle_placeholder(ui, app.index_len())
        };
    }
    if let Some(error) = app.lanes.get("search").filter(|lane| lane.error) {
        let message = error.status.clone();
        ui.centered_and_justified(|ui| {
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new("Search could not complete")
                        .font(sans(15.0))
                        .strong(),
                );
                ui.add(
                    egui::Label::new(RichText::new(message).font(sans(11.0)).color(MUTED)).wrap(),
                );
                if secondary_button(ui, "Retry search", MUTED).clicked() {
                    app.requery();
                }
            });
        });
        return;
    }
    let invalid_regex = app.regex_mode
        && !app.query.is_empty()
        && RegexBuilder::new(&app.query)
            .case_insensitive(!app.case_sensitive)
            .build()
            .is_err();
    ui.centered_and_justified(|ui| {
        ui.vertical_centered(|ui| {
            paint_search_icon(ui, if invalid_regex { ERROR } else { SUBTLE });
            ui.add_space(7.0);
            ui.label(
                RichText::new(if invalid_regex {
                    "Invalid regular expression"
                } else {
                    "No matching objects"
                })
                .font(sans(15.0))
                .strong(),
            );
            ui.label(
                RichText::new(if invalid_regex {
                    "Edit the expression or reset search options."
                } else if app.regex_mode {
                    "No files match this regular expression."
                } else {
                    "Try fewer words or search a different location."
                })
                .font(sans(11.0))
                .color(MUTED),
            );
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let action_width = if app.query.is_empty() { 132.0 } else { 222.0 };
                ui.add_space((ui.available_width() - action_width).max(0.0) * 0.5);
                if !app.query.is_empty() && secondary_button(ui, "Clear search", MUTED).clicked() {
                    app.query.clear();
                    app.requery();
                }
                let has_options = app.regex_mode
                    || app.case_sensitive
                    || app.whole_word
                    || app.ignore_accents
                    || app.search_mode != SearchMode::Name
                    || app.kind_filter != KindFilter::All
                    || app.scope_root.is_some();
                if has_options && secondary_button(ui, "Reset search options", MUTED).clicked() {
                    app.regex_mode = false;
                    app.case_sensitive = false;
                    app.whole_word = false;
                    app.ignore_accents = false;
                    app.search_mode = SearchMode::Name;
                    app.kind_filter = KindFilter::All;
                    app.scope_root = None;
                    app.save_settings();
                    app.requery();
                }
            });
        });
    });
}
