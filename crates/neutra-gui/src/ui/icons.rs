//! Drawn filter-pill icons and the pill button itself. Vector shapes, not
//! font glyphs, so they render identically everywhere.

use super::widgets::{ACID_STRONG, MUTED, SURFACE, TEXT, sans};
use super::KindFilter;
use super::sidebar::SidebarTab;
use egui::{Color32, Rect, RichText, Sense, Stroke, StrokeKind, Vec2};

pub(super) const GREEN: Color32 = Color32::from_rgb(63, 185, 80);

pub(super) fn preset_pill(ui: &mut egui::Ui, preset: KindFilter, active: bool) -> egui::Response {
    let text_color = if active { TEXT } else { MUTED };
    let label = if preset == KindFilter::All {
        preset.label().to_owned()
    } else {
        format!("    {}", preset.label())
    };
    let response = ui.add(
        egui::Button::new(RichText::new(label).font(sans(10.0)).color(text_color))
            .fill(if active { ACID_STRONG } else { SURFACE })
            .stroke(Stroke::new(
                1.0_f32,
                if active { ACID_STRONG } else { Color32::TRANSPARENT },
            ))
            .corner_radius(14)
            .min_size(Vec2::new(0.0, 28.0)),
    );
    if preset != KindFilter::All {
        let icon = Rect::from_min_size(
            response.rect.left_top() + Vec2::new(9.0, 7.0),
            Vec2::splat(14.0),
        );
        paint_preset_glyph(ui.painter(), icon, preset, text_color);
    }
    response
}

fn paint_preset_glyph(p: &egui::Painter, rect: Rect, preset: KindFilter, color: Color32) {
    let stroke = Stroke::new(1.4_f32, color);
    match preset {
        KindFilter::All | KindFilter::Files => {}
        KindFilter::Audio => {
            p.line_segment([rect.left_top() + Vec2::new(8.5, 1.5), rect.left_top() + Vec2::new(8.5, 10.0)], stroke);
            p.line_segment([rect.left_top() + Vec2::new(8.5, 1.5), rect.left_top() + Vec2::new(12.0, 3.5)], stroke);
            p.circle_filled(rect.left_top() + Vec2::new(6.0, 10.5), 2.2, color);
        }
        KindFilter::Images => {
            p.rect_stroke(rect.shrink(1.5), 1.0, stroke, StrokeKind::Inside);
            p.circle_filled(rect.left_top() + Vec2::new(4.5, 5.0), 1.3, color);
            p.line_segment([rect.left_bottom() + Vec2::new(1.5, -1.5), rect.center() + Vec2::new(-1.0, 1.5)], stroke);
            p.line_segment([rect.center() + Vec2::new(-1.0, 1.5), rect.center() + Vec2::new(1.5, -1.0)], stroke);
            p.line_segment([rect.center() + Vec2::new(1.5, -1.0), rect.right_bottom() + Vec2::new(-1.5, -1.5)], stroke);
        }
        KindFilter::Video => {
            p.rect_stroke(rect.shrink(1.5), 1.0, stroke, StrokeKind::Inside);
            let c = rect.center();
            p.add(egui::Shape::convex_polygon(vec![c + Vec2::new(-2.0, -3.0), c + Vec2::new(-2.0, 3.0), c + Vec2::new(3.0, 0.0)], color, Stroke::NONE));
        }
        KindFilter::Programs => {
            for (dx, dy) in [(3.0, 3.0), (8.0, 3.0), (3.0, 8.0), (8.0, 8.0)] {
                p.rect_filled(Rect::from_min_size(rect.left_top() + Vec2::new(dx, dy), Vec2::splat(3.0)), 0.5, color);
            }
        }
        KindFilter::Compressed => {
            p.rect_stroke(Rect::from_min_size(rect.left_top() + Vec2::new(2.0, 3.5), Vec2::new(10.0, 8.0)), 1.0, stroke, StrokeKind::Inside);
            p.line_segment([rect.left_top() + Vec2::new(5.5, 3.5), rect.left_top() + Vec2::new(5.5, 11.5)], stroke);
            p.line_segment([rect.left_top() + Vec2::new(8.5, 3.5), rect.left_top() + Vec2::new(8.5, 11.5)], stroke);
        }
        KindFilter::Documents => {
            p.rect_stroke(Rect::from_min_size(rect.left_top() + Vec2::new(3.5, 1.5), Vec2::new(7.0, 11.0)), 1.0, stroke, StrokeKind::Inside);
            p.line_segment([rect.left_top() + Vec2::new(5.0, 5.5), rect.left_top() + Vec2::new(9.0, 5.5)], stroke);
            p.line_segment([rect.left_top() + Vec2::new(5.0, 8.0), rect.left_top() + Vec2::new(9.0, 8.0)], stroke);
        }
        KindFilter::Folders => {
            p.rect_stroke(Rect::from_min_size(rect.left_top() + Vec2::new(1.5, 4.0), Vec2::new(11.0, 8.0)), 1.0, stroke, StrokeKind::Inside);
            p.line_segment([rect.left_top() + Vec2::new(1.5, 4.0), rect.left_top() + Vec2::new(6.0, 1.5)], stroke);
        }
    }
}

/// Sidebar tab glyphs. Reuses the folder preset glyph for locations.
pub(super) fn tab_icon(ui: &mut egui::Ui, tab: SidebarTab, color: Color32) {
    match tab {
        SidebarTab::Locations => {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
            paint_preset_glyph(ui.painter(), rect, KindFilter::Folders, color);
        }
        SidebarTab::Status => {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
            let p = ui.painter();
            let stroke = Stroke::new(1.6_f32, color);
            for (i, h) in [5.0, 9.0, 6.5].into_iter().enumerate() {
                let x = rect.left_top().x + 2.5 + i as f32 * 3.5;
                p.line_segment(
                    [egui::pos2(x, rect.bottom() - 1.5), egui::pos2(x, rect.bottom() - 1.5 - h)],
                    stroke,
                );
            }
        }
        SidebarTab::Scanner => {
            super::widgets::paint_search_icon(ui, color);
        }
        SidebarTab::Maintenance => {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
            let p = ui.painter();
            let stroke = Stroke::new(1.4_f32, color);
            p.circle_stroke(rect.left_top() + Vec2::new(4.5, 4.5), 2.6, stroke);
            p.line_segment([rect.left_top() + Vec2::new(6.5, 6.5), rect.right_bottom() + Vec2::new(-1.5, -1.5)], stroke);
        }
        SidebarTab::Network => {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
            let p = ui.painter();
            let a = rect.left_top() + Vec2::new(3.0, 3.0);
            let b = rect.left_top() + Vec2::new(11.0, 3.0);
            let c = rect.left_top() + Vec2::new(7.0, 11.0);
            let stroke = Stroke::new(1.2_f32, color);
            p.line_segment([a, b], stroke);
            p.line_segment([a, c], stroke);
            p.line_segment([b, c], stroke);
            for point in [a, b, c] {
                p.circle_filled(point, 1.8, color);
            }
        }
    }
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
    p.rect_filled(rect, 2.0, if active { super::widgets::ACTIVE } else { super::widgets::SURFACE });
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
