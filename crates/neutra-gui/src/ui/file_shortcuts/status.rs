use super::*;

pub(super) fn show(app: &mut NeutraApp, ui: &Ui) {
    let Some((message, error)) = &app.file_operations.message else {
        return;
    };
    let message = message.clone();
    let color = if *error { ACID } else { TEXT };
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
                            egui::Label::new(RichText::new(message).color(color))
                                .halign(egui::Align::Min)
                                .wrap(),
                        );
                        if ui.small_button("Dismiss").clicked() {
                            app.file_operations.message = None;
                        }
                    });
                });
        });
}
