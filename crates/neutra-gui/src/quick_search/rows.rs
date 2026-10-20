use crate::ui::widgets::*;
use egui::{Align2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};
use neutra_core::SearchHit;

pub(super) fn result_row(ui: &mut Ui, hit: &SearchHit, selected: bool) -> bool {
    let record = &hit.record;
    crate::ui::script_fonts::ensure(ui.ctx(), &record.path);
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 44.0), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            true,
            selected,
            format!("{}\n{}", record.name(), record.path),
        )
    });
    let painter = ui.painter();
    painter.rect_filled(
        rect,
        4,
        if selected {
            SELECTED
        } else if response.hovered() {
            HOVER
        } else {
            SURFACE
        },
    );
    if selected {
        painter.rect_filled(
            Rect::from_min_size(rect.min + Vec2::new(0.0, 6.0), Vec2::new(3.0, 32.0)),
            1,
            ACID_STRONG,
        );
    }
    if response.has_focus() {
        painter.rect_stroke(rect, 4, Stroke::new(2.0_f32, ACID), StrokeKind::Inside);
    }
    let badge = Rect::from_center_size(
        rect.left_center() + Vec2::new(27.0, 0.0),
        Vec2::new(28.0, 24.0),
    );
    painter.rect_stroke(
        badge,
        4,
        Stroke::new(1.0_f32, type_color(record)),
        StrokeKind::Inside,
    );
    painter.text(
        badge.center(),
        Align2::CENTER_CENTER,
        type_badge(record),
        mono(9.0),
        TEXT,
    );
    let left = badge.right() + 12.0;
    let width = (rect.right() - left - 12.0).max(0.0);
    let name = painter.layout(record.name().to_owned(), sans(SMALL), TEXT, f32::INFINITY);
    let path = painter.layout(record.path.to_string(), mono(CAPTION), MUTED, f32::INFINITY);
    let text_rect = Rect::from_min_size(
        egui::pos2(left, rect.top()),
        Vec2::new(width, rect.height()),
    );
    let painter = painter.with_clip_rect(ui.clip_rect().intersect(text_rect));
    painter.galley(egui::pos2(left, rect.top() + 5.0), name, TEXT);
    painter.galley(egui::pos2(left, rect.top() + 24.0), path, MUTED);
    if selected
        && ui.input(|input| {
            input.key_pressed(egui::Key::ArrowDown) || input.key_pressed(egui::Key::ArrowUp)
        })
    {
        response.scroll_to_me(None);
    }
    let activate = response.has_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
    response.on_hover_text(record.path.as_ref()).clicked() || activate
}
