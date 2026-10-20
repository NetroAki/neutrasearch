use super::*;
use crate::transport::file_actions::Action;

pub(super) fn update(app: &mut NeutraApp, ui: &mut Ui) {
    if ui.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::A)) {
        let order: Vec<_> = app
            .hits
            .iter()
            .map(|hit| hit.record.path.as_ref())
            .collect();
        app.selection.all(&order);
        app.selected = order.first().map(|path| (*path).into());
    }
    if ui.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::Z)) {
        app.file_operations.undo();
    }
    let Some(path) = app.selected.as_ref().map(PathBuf::from) else {
        return;
    };
    let paths = app.selection.paths();
    let key =
        |ui: &mut Ui, key| ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, key));
    let action = if key(ui, egui::Key::F2) {
        (paths.len() == 1).then(|| Action::Rename {
            from: paths[0].clone(),
            to: paths[0].clone(),
        })
    } else if key(ui, egui::Key::Delete) {
        (!paths.is_empty()).then(|| Action::TrashMany(paths.clone()))
    } else if clipboard::consume(ui, egui::Key::C) {
        (!paths.is_empty()).then(|| Action::Copy(paths.clone()))
    } else if clipboard::consume(ui, egui::Key::X) {
        (!paths.is_empty()).then_some(Action::Cut(paths))
    } else if clipboard::consume(ui, egui::Key::V) {
        Some(Action::Paste(path))
    } else {
        None
    };
    if let Some(action) = action {
        app.file_operations.dispatch(action);
    }
}
