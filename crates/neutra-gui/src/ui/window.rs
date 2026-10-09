use super::*;

pub(super) fn controls(ui: &mut Ui) {
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        for (label, help, command) in [
            ("×", "Close", egui::ViewportCommand::Close),
            (
                "□",
                "Maximise or restore",
                egui::ViewportCommand::Maximized(
                    !ui.input(|input| input.viewport().maximized.unwrap_or(false)),
                ),
            ),
            ("−", "Minimise", egui::ViewportCommand::Minimized(true)),
        ] {
            if ui
                .add(
                    egui::Button::new(RichText::new(label).size(17.0))
                        .frame(false)
                        .min_size(Vec2::new(36.0, 28.0)),
                )
                .on_hover_text(help)
                .clicked()
            {
                ui.ctx().send_viewport_cmd(command);
            }
        }
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.0), Sense::drag());
        if response.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if response.double_clicked() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(
                !ui.input(|input| input.viewport().maximized.unwrap_or(false)),
            ));
        }
        let _ = rect;
    });
}

pub(super) fn resize(ui: &mut Ui) {
    if ui.input(|input| input.viewport().maximized.unwrap_or(false)) {
        return;
    }
    let rect = ui.max_rect();
    let edge = 5.0;
    use egui::ResizeDirection as D;
    let regions = [
        (
            D::NorthWest,
            Rect::from_min_size(rect.min, Vec2::splat(edge * 2.0)),
        ),
        (
            D::NorthEast,
            Rect::from_min_size(
                rect.right_top() - Vec2::new(edge * 2.0, 0.0),
                Vec2::splat(edge * 2.0),
            ),
        ),
        (
            D::SouthWest,
            Rect::from_min_size(
                rect.left_bottom() - Vec2::new(0.0, edge * 2.0),
                Vec2::splat(edge * 2.0),
            ),
        ),
        (
            D::SouthEast,
            Rect::from_min_size(rect.max - Vec2::splat(edge * 2.0), Vec2::splat(edge * 2.0)),
        ),
        (
            D::North,
            Rect::from_min_max(
                rect.min + Vec2::new(edge * 2.0, 0.0),
                rect.right_top() + Vec2::new(-edge * 2.0, edge),
            ),
        ),
        (
            D::South,
            Rect::from_min_max(
                rect.left_bottom() + Vec2::new(edge * 2.0, -edge),
                rect.max - Vec2::new(edge * 2.0, 0.0),
            ),
        ),
        (
            D::West,
            Rect::from_min_max(
                rect.min + Vec2::new(0.0, edge * 2.0),
                rect.left_bottom() + Vec2::new(edge, -edge * 2.0),
            ),
        ),
        (
            D::East,
            Rect::from_min_max(
                rect.right_top() + Vec2::new(-edge, edge * 2.0),
                rect.max - Vec2::new(0.0, edge * 2.0),
            ),
        ),
    ];
    for (direction, region) in regions {
        let cursor = match direction {
            D::North | D::South => egui::CursorIcon::ResizeVertical,
            D::West | D::East => egui::CursorIcon::ResizeHorizontal,
            D::NorthWest | D::SouthEast => egui::CursorIcon::ResizeNwSe,
            _ => egui::CursorIcon::ResizeNeSw,
        };
        let response = ui
            .interact(
                region,
                Id::new(("window-resize", format!("{direction:?}"))),
                Sense::drag(),
            )
            .on_hover_cursor(cursor);
        if response.drag_started() {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
        }
    }
}
