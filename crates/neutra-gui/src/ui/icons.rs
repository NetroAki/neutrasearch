//! Small drawn glyphs (drive, database, view toggles) and the text tab
//! with its underline indicator. Vector shapes, not font glyphs.

use super::widgets::{ACID, GLOW, LINE_STRONG, MICRO, MUTED, sans, tracked};
use egui::{Color32, Rect, RichText, Sense, Stroke, StrokeKind, Vec2};

pub(super) use super::widgets::GREEN;

/// Pill chip, as in the mockup's browser filters: uppercase micro label, an
/// accent-glow outline and tint when active.
pub(super) fn filter_tab(ui: &mut egui::Ui, label: &str, active: bool) -> egui::Response {
    let (text, fill, outline) = if active {
        (ACID, GLOW.gamma_multiply(0.14), GLOW)
    } else {
        (MUTED, Color32::TRANSPARENT, LINE_STRONG)
    };
    ui.add(
        egui::Button::new(RichText::new(tracked(label)).font(sans(MICRO)).strong().color(text))
            .fill(fill)
            .stroke(Stroke::new(1.0_f32, outline))
            .corner_radius(12)
            .min_size(Vec2::new(0.0, 24.0)),
    )
}

/// House glyph for the home location row.
pub(super) fn paint_home_icon(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
    let p = ui.painter();
    let stroke = Stroke::new(1.4_f32, color);
    let top = rect.left_top() + Vec2::new(7.0, 2.5);
    p.line_segment([rect.left_top() + Vec2::new(1.5, 7.0), top], stroke);
    p.line_segment([top, rect.right_top() + Vec2::new(-1.5, 7.0)], stroke);
    p.rect_stroke(
        Rect::from_min_size(rect.left_top() + Vec2::new(3.5, 7.0), Vec2::new(7.0, 5.5)),
        0.5,
        stroke,
        StrokeKind::Inside,
    );
}

/// Drive glyph for per-drive rows.
pub(super) fn paint_drive_icon(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
    let p = ui.painter();
    let stroke = Stroke::new(1.4_f32, color);
    p.rect_stroke(Rect::from_min_size(rect.left_top() + Vec2::new(1.5, 3.0), Vec2::new(11.0, 7.0)), 1.0, stroke, StrokeKind::Inside);
    p.line_segment([rect.left_top() + Vec2::new(1.5, 12.0), rect.right_top() + Vec2::new(-1.5, 12.0)], stroke);
}

/// Database glyph for the status bar and indexing card.
pub(super) fn paint_db_icon(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
    let p = ui.painter();
    let stroke = Stroke::new(1.3_f32, color);
    let top = rect.left_top() + Vec2::new(3.0, 3.0);
    p.add(egui::Shape::ellipse_stroke(top + Vec2::new(5.0, 0.0), Vec2::new(5.0, 2.0), stroke));
    p.line_segment([top + Vec2::new(0.0, 0.0), top + Vec2::new(0.0, 10.0)], stroke);
    p.line_segment([top + Vec2::new(10.0, 0.0), top + Vec2::new(10.0, 10.0)], stroke);
    p.add(egui::Shape::ellipse_stroke(top + Vec2::new(5.0, 10.0), Vec2::new(5.0, 2.0), stroke));
}

/// List/grid view toggle button for the results toolbar.
pub(super) fn view_button(ui: &mut egui::Ui, list: bool, active: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(30.0, 25.0), Sense::click());
    let p = ui.painter();
    p.rect_filled(rect, 4.0, if active { super::widgets::SELECTED } else { super::widgets::RAISED });
    p.rect_stroke(rect, 4.0, Stroke::new(1.0_f32, super::widgets::LINE_STRONG), StrokeKind::Inside);
    let color = if active { super::widgets::ACID } else { MUTED };
    let stroke = Stroke::new(1.4_f32, color);
    if list {
        for y in [0.0, 4.5, 9.0] {
            p.line_segment(
                [rect.left_top() + Vec2::new(8.0, 8.0 + y), rect.left_top() + Vec2::new(22.0, 8.0 + y)],
                stroke,
            );
        }
    } else {
        for (dx, dy) in [(0.0, 0.0), (7.0, 0.0), (0.0, 7.0), (7.0, 7.0)] {
            p.rect_stroke(
                Rect::from_min_size(rect.left_top() + Vec2::new(8.0 + dx, 5.5 + dy), Vec2::splat(5.5)),
                1.0,
                stroke,
                StrokeKind::Inside,
            );
        }
    }
    response
}
