use super::*;
use crate::transport::file_actions::Action;

pub(super) fn update(app: &mut NeutraApp, ui: &mut Ui) {
    if app.file_operations.update(ui.ctx()) {
        app.requery();
        app.tree_model = None;
        app.map_file_key = None;
    }
    if !ui.ctx().egui_wants_keyboard_input() {
        if ui.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::Z)) {
            app.file_operations.undo();
        }
        if let Some(path) = app.selected.as_ref().map(PathBuf::from) {
            let key = |ui: &mut Ui, modifiers, key| {
                ui.input_mut(|input| input.consume_key(modifiers, key))
            };
            let action = if key(ui, egui::Modifiers::NONE, egui::Key::F2) {
                Some(Action::Rename {
                    from: path.clone(),
                    to: path.clone(),
                })
            } else if key(ui, egui::Modifiers::NONE, egui::Key::Delete) {
                Some(Action::Trash(path.clone()))
            } else if key(ui, egui::Modifiers::COMMAND, egui::Key::C) {
                Some(Action::Copy(vec![path.clone()]))
            } else if key(ui, egui::Modifiers::COMMAND, egui::Key::X) {
                Some(Action::Cut(vec![path.clone()]))
            } else if key(ui, egui::Modifiers::COMMAND, egui::Key::V) {
                Some(Action::Paste(path))
            } else {
                None
            };
            if let Some(action) = action {
                app.file_operations.dispatch(action);
            }
        }
    }
    if let Some((message, error)) = &app.file_operations.message {
        let message = message.clone();
        let error = *error;
        egui::Area::new(Id::new("file-operation-status"))
            .anchor(Align2::RIGHT_BOTTOM, Vec2::new(-12.0, -32.0))
            .show(ui.ctx(), |ui| {
                egui::Frame::new()
                    .fill(SURFACE)
                    .stroke(Stroke::new(1.0_f32, LINE_STRONG))
                    .inner_margin(8)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.add_sized(
                                [340.0, 0.0],
                                egui::Label::new(RichText::new(message).color(if error {
                                    ACID
                                } else {
                                    TEXT
                                }))
                                .wrap(),
                            );
                            if ui.small_button("Dismiss").clicked() {
                                app.file_operations.message = None;
                            }
                        });
                    });
            });
    }
    super::file_actions::dialogs(&mut app.file_operations, ui.ctx());
}
