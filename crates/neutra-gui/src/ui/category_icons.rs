use super::*;

pub(super) fn button(ui: &mut Ui, kind: KindFilter, active: bool) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(
        kind.label().into(),
        sans(SMALL),
        if active { ACID } else { MUTED },
    );
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(galley.size().x + 32.0, 28.0), Sense::click());
    let color = if active || response.hovered() {
        ACID
    } else {
        MUTED
    };
    if active || response.hovered() {
        ui.painter()
            .rect_filled(rect, 4.0, if active { SELECTED } else { HOVER });
    }
    if response.has_focus() {
        ui.painter()
            .rect_stroke(rect, 4.0, Stroke::new(2.0_f32, GLOW), StrokeKind::Inside);
    }
    paint(
        ui,
        Rect::from_center_size(rect.left_center() + Vec2::new(12.0, 0.0), Vec2::splat(14.0)),
        kind,
        color,
    );
    ui.painter().galley(
        rect.left_center() + Vec2::new(24.0, -galley.size().y / 2.0),
        galley,
        color,
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            true,
            active,
            kind.label(),
        )
    });
    response
}

fn paint(ui: &Ui, rect: Rect, kind: KindFilter, color: Color32) {
    let painter = ui.painter();
    let stroke = Stroke::new(1.2_f32, color);
    let point = |x, y| rect.min + Vec2::new(x, y);
    match kind {
        KindFilter::All => {
            for (x, y) in [(1.0, 1.0), (8.0, 1.0), (1.0, 8.0), (8.0, 8.0)] {
                painter.rect_stroke(
                    Rect::from_min_size(point(x, y), Vec2::splat(4.0)),
                    0.0,
                    stroke,
                    StrokeKind::Inside,
                );
            }
        }
        KindFilter::Audio => {
            painter.line_segment([point(5.0, 11.0), point(5.0, 3.0)], stroke);
            painter.line_segment([point(5.0, 3.0), point(12.0, 1.0)], stroke);
            painter.line_segment([point(12.0, 1.0), point(12.0, 9.0)], stroke);
            painter.circle_filled(point(3.0, 11.0), 2.0, color);
            painter.circle_filled(point(10.0, 9.0), 2.0, color);
        }
        KindFilter::Folders => {
            painter.line_segment([point(1.0, 4.0), point(1.0, 2.0)], stroke);
            painter.line_segment([point(1.0, 2.0), point(6.0, 2.0)], stroke);
            painter.rect_stroke(
                Rect::from_min_max(point(1.0, 4.0), point(13.0, 12.0)),
                1.0,
                stroke,
                StrokeKind::Inside,
            );
        }
        _ => {
            painter.rect_stroke(
                Rect::from_min_max(point(1.0, 1.0), point(13.0, 13.0)),
                1.0,
                stroke,
                StrokeKind::Inside,
            );
            match kind {
                KindFilter::Images => {
                    painter.circle_filled(point(9.0, 4.0), 1.5, color);
                    painter.line_segment([point(2.0, 11.0), point(6.0, 6.0)], stroke);
                    painter.line_segment([point(6.0, 6.0), point(12.0, 11.0)], stroke);
                }
                KindFilter::Video => {
                    painter.add(egui::Shape::convex_polygon(
                        vec![point(5.0, 4.0), point(10.0, 7.0), point(5.0, 10.0)],
                        color,
                        Stroke::NONE,
                    ));
                }
                KindFilter::Programs => {
                    painter.line_segment([point(3.0, 4.0), point(6.0, 7.0)], stroke);
                    painter.line_segment([point(6.0, 7.0), point(3.0, 10.0)], stroke);
                    painter.line_segment([point(8.0, 10.0), point(11.0, 10.0)], stroke);
                }
                KindFilter::Compressed => {
                    for y in [3.0, 6.0, 9.0] {
                        painter.line_segment([point(6.0, y), point(8.0, y)], stroke);
                    }
                }
                _ => {
                    for y in [4.0, 7.0, 10.0] {
                        painter.line_segment([point(4.0, y), point(10.0, y)], stroke);
                    }
                }
            }
        }
    }
}
