use super::*;

pub(super) fn focus_clicked(app: &mut NeutraApp, response: &egui::Response) {
    response.request_focus();
    app.file_focus = Some(response.id);
}

pub(super) fn result_context_menu(app: &mut NeutraApp, response: &egui::Response, path: &str) {
    if response.secondary_clicked() && !app.selection.contains(path) {
        app.select_single(path);
    }
    let paths = app.selection.paths();
    let mut action = None;
    super::file_actions::popup(response, |menu| {
        super::file_actions::selection_menu(menu, path, &paths, &mut action)
    });
    if let Some(action) = action {
        app.file_operations.dispatch(action);
    }
}

pub(super) fn row_info(
    app: &mut NeutraApp,
    ui: &Ui,
    response: &egui::Response,
    record: &FileRecord,
) -> bool {
    let path = record.path.as_ref();
    if response.has_focus() {
        app.file_focus = Some(response.id);
        if response.gained_focus() && app.selected.as_deref() != Some(path) {
            app.select_single(path);
        }
    }
    let selected = app.selection.contains(path);
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            true,
            selected,
            format!(
                "{}; {}; {}",
                record.name(),
                format_size(record.disk_bytes()),
                path
            ),
        )
    });
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_role(egui::accesskit::Role::ListBoxOption);
        node.set_selected(selected);
        node.clear_toggled();
    });
    selected
}
