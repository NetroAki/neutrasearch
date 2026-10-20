use super::*;

pub(super) struct DetailColumns {
    pub(super) name: Rect,
    pub(super) path: Rect,
    pub(super) modified: Rect,
    pub(super) size: Rect,
}

pub(super) fn detail_columns(rect: Rect) -> DetailColumns {
    let name_w = rect.width() * 0.32;
    let path_w = rect.width() * 0.39;
    let modified_w = rect.width() * 0.17;
    let size_w = rect.width() - name_w - path_w - modified_w;
    let mut x = rect.left();
    let name = Rect::from_min_size(egui::pos2(x, rect.top()), Vec2::new(name_w, rect.height()));
    x += name_w;
    let path = Rect::from_min_size(egui::pos2(x, rect.top()), Vec2::new(path_w, rect.height()));
    x += path_w;
    let modified = Rect::from_min_size(
        egui::pos2(x, rect.top()),
        Vec2::new(modified_w, rect.height()),
    );
    x += modified_w;
    let size = Rect::from_min_size(egui::pos2(x, rect.top()), Vec2::new(size_w, rect.height()));
    DetailColumns {
        name,
        path,
        modified,
        size,
    }
}

/// Draw single-line text with matched byte ranges emphasized. Falls back to
/// plain text when a range is not a char boundary (exotic case-fold shifts).
pub(super) struct NameStyle {
    pub(super) font: FontId,
    pub(super) color: Color32,
    pub(super) highlight: Color32,
}

pub(super) fn paint_highlighted(
    ui: &Ui,
    pos: egui::Pos2,
    clip: Rect,
    text: &str,
    ranges: &[std::ops::Range<usize>],
    style: &NameStyle,
) {
    let NameStyle {
        font,
        color,
        highlight,
    } = style;
    let painter = ui.painter().with_clip_rect(clip);
    let clean = ranges.iter().all(|r| {
        text.get(r.clone()).is_some()
            && text.is_char_boundary(r.start)
            && text.is_char_boundary(r.end)
    });
    if ranges.is_empty() || !clean {
        painter.text(pos, Align2::LEFT_CENTER, text, font.clone(), *color);
        return;
    }
    let mut job = egui::text::LayoutJob {
        wrap: egui::text::TextWrapping {
            max_width: f32::INFINITY,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut cursor = 0usize;
    for range in ranges {
        let start = range.start.max(cursor);
        let end = range.end.min(text.len());
        if start > cursor {
            job.append(
                &text[cursor..start],
                0.0,
                egui::text::TextFormat::simple(font.clone(), *color),
            );
        }
        if end > start {
            let mut format = egui::text::TextFormat::simple(font.clone(), *highlight);
            format.underline = Stroke::new(1.0_f32, *highlight);
            job.append(&text[start..end], 0.0, format);
        }
        cursor = end.max(cursor);
    }
    if cursor < text.len() {
        job.append(
            &text[cursor..],
            0.0,
            egui::text::TextFormat::simple(font.clone(), *color),
        );
    }
    let galley = painter.layout_job(job);
    let top = pos.y - galley.size().y * 0.5;
    painter.galley(egui::pos2(pos.x, top), galley, *color);
}

pub(super) fn paint_details_row(
    ui: &mut Ui,
    rect: Rect,
    record: &neutra_core::FileRecord,
    selected: bool,
    ranges: &[std::ops::Range<usize>],
) {
    let hovered = ui.rect_contains_pointer(rect);
    let fill = if selected {
        SELECTED
    } else if hovered {
        HOVER
    } else {
        CANVAS
    };
    ui.painter().rect_filled(rect, 0.0, fill);
    ui.painter()
        .hline(rect.x_range(), rect.bottom(), Stroke::new(1.0_f32, LINE));
    if selected {
        ui.painter().rect_filled(
            Rect::from_min_size(rect.min, Vec2::new(3.0, rect.height())),
            0.0,
            ACID_STRONG,
        );
    }
    let columns = detail_columns(rect);
    let badge = Rect::from_center_size(
        columns.name.left_center() + Vec2::new(21.0, 0.0),
        Vec2::new(28.0, 20.0),
    );
    let badge_color = type_color(record);
    ui.painter().rect_filled(badge, 4.0, RAISED);
    ui.painter().rect_stroke(
        badge,
        4.0,
        Stroke::new(1.0_f32, badge_color),
        StrokeKind::Inside,
    );
    ui.painter().text(
        badge.center(),
        Align2::CENTER_CENTER,
        type_badge(record),
        mono(9.0),
        TEXT,
    );
    paint_highlighted(
        ui,
        egui::pos2(badge.right() + 6.0, columns.name.center().y),
        Rect::from_min_max(
            egui::pos2(badge.right() + 6.0, columns.name.top()),
            columns.name.max,
        ),
        record.name(),
        ranges,
        &NameStyle {
            font: sans(SMALL),
            color: TEXT,
            highlight: ACID,
        },
    );
    let metadata = if selected { TEXT } else { MUTED };
    ui.painter()
        .with_clip_rect(columns.path.shrink2(Vec2::new(7.0, 0.0)))
        .text(
            columns.path.left_center() + Vec2::new(7.0, 0.0),
            Align2::LEFT_CENTER,
            shorten(
                &parent_path(&record.path),
                (((columns.path.width() - 20.0) / 6.6) as usize).max(16),
            ),
            mono(CAPTION),
            metadata,
        );
    ui.painter().text(
        columns.modified.left_center() + Vec2::new(7.0, 0.0),
        Align2::LEFT_CENTER,
        format_mtime(record.mtime),
        sans(SMALL),
        metadata,
    );
    ui.painter().text(
        columns.size.right_center() - Vec2::new(7.0, 0.0),
        Align2::RIGHT_CENTER,
        format_size(record.disk_bytes()),
        mono(CAPTION),
        metadata,
    );
}
