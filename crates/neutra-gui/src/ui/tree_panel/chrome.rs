use super::*;

pub(super) fn header(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, SURFACE);
    ui.painter().hline(
        rect.x_range(),
        rect.bottom(),
        Stroke::new(1.0_f32, LINE_STRONG),
    );
    let columns = row_layout(rect, 0);
    let label = |x: f32, align: Align2, text: &str| {
        ui.painter().text(
            egui::pos2(x, rect.center().y),
            align,
            tracked(text),
            sans(MICRO),
            MUTED,
        );
    };
    label(rect.left() + 32.0, Align2::LEFT_CENTER, "Name");
    if let Some(bar_right) = columns.bar_right {
        label(
            bar_right - columns.bar_width * 0.5,
            Align2::CENTER_CENTER,
            "Share",
        );
    }
    if let Some(size_right) = columns.size_right {
        label(size_right, Align2::RIGHT_CENTER, "Size");
    }
    if let Some(items_right) = columns.items_right {
        label(items_right, Align2::RIGHT_CENTER, "Items");
    }
}

pub(super) fn chevron(ui: &Ui, centre: egui::Pos2, open: bool, hot: bool) {
    let points = if open {
        vec![
            centre + Vec2::new(-4.0, -2.0),
            centre + Vec2::new(4.0, -2.0),
            centre + Vec2::new(0.0, 3.0),
        ]
    } else {
        vec![
            centre + Vec2::new(-2.0, -4.0),
            centre + Vec2::new(-2.0, 4.0),
            centre + Vec2::new(3.0, 0.0),
        ]
    };
    ui.painter().add(egui::Shape::convex_polygon(
        points,
        if hot { TEXT } else { MUTED },
        Stroke::NONE,
    ));
}

pub(super) fn folder_glyph(ui: &Ui, centre: egui::Pos2) {
    let body = Rect::from_center_size(centre + Vec2::new(0.0, 1.0), Vec2::new(12.0, 8.0));
    ui.painter()
        .rect_stroke(body, 2.0, Stroke::new(1.0_f32, WARN), StrokeKind::Inside);
    ui.painter().line_segment(
        [
            body.left_top() + Vec2::new(0.0, -1.5),
            body.left_top() + Vec2::new(4.5, -1.5),
        ],
        Stroke::new(1.5_f32, WARN),
    );
}
