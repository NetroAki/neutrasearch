use super::*;

mod clipboard;
mod keyboard;
mod status;

pub(super) fn allowed(app: &NeutraApp, ui: &Ui) -> bool {
    if app.file_operations.rename.is_some() || app.file_operations.open_with.is_some() {
        return false;
    }
    !ui.ctx().egui_wants_keyboard_input()
        || app
            .file_focus
            .is_some_and(|id| ui.memory(|memory| memory.focused()) == Some(id))
}

pub(super) fn update(app: &mut NeutraApp, ui: &mut Ui) {
    let restore_key = Id::new("restore-file-focus");
    if let Some(id) = ui
        .ctx()
        .data_mut(|data| data.remove_temp::<Option<Id>>(restore_key).flatten())
    {
        ui.memory_mut(|memory| memory.request_focus(id));
    }
    if app.file_operations.update(ui.ctx()) {
        app.last_query = None;
        app.requery();
        app.tree_model = None;
        app.map_file_key = None;
    }
    if allowed(app, ui) {
        keyboard::update(app, ui);
    }
    status::show(app, ui);
    let dialog_open =
        app.file_operations.rename.is_some() || app.file_operations.open_with.is_some();
    super::file_actions::dialogs(&mut app.file_operations, ui.ctx());
    if dialog_open
        && app.file_operations.rename.is_none()
        && app.file_operations.open_with.is_none()
    {
        if let Some(id) = app.file_focus {
            ui.ctx()
                .data_mut(|data| data.insert_temp(restore_key, Some(id)));
            ui.ctx().request_repaint();
        }
    }
}
