use super::adapters::{paint_highlighted, NameStyle};
use super::*;

pub(in crate::ui) fn list_view(app: &mut NeutraApp, ui: &mut Ui) {
    if app.hits.is_empty() {
        empty_results(app, ui);
        return;
    }
    let row_h = 28.0;
    let rows = ((ui.available_height() - 8.0) / row_h).floor().max(1.0) as usize;
    let columns = app.hits.len().div_ceil(rows);
    let col_w = 240.0;
    let mut open_path = None;
    let selection_changed = keyboard_selection(app, ui, &mut open_path);
    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let (canvas, _) = ui.allocate_exact_size(
                Vec2::new(columns as f32 * col_w, rows as f32 * row_h),
                Sense::hover(),
            );
            // Paint only cells inside the viewport. Previously every hit got
            // a widget every frame, which froze List view on large indexes.
            let clip = ui.clip_rect().intersect(canvas);
            if clip.width() <= 0.0 || clip.height() <= 0.0 {
                return;
            }
            let first_col = (((clip.left() - canvas.left()) / col_w).floor().max(0.0)) as usize;
            let last_col =
                (((clip.right() - canvas.left()) / col_w).ceil().max(1.0) as usize).min(columns);
            let first_row = (((clip.top() - canvas.top()) / row_h).floor().max(0.0)) as usize;
            let last_row =
                (((clip.bottom() - canvas.top()) / row_h).ceil().max(1.0) as usize).min(rows);
            for col in first_col..last_col {
                for row in first_row..last_row {
                    let position = col * rows + row;
                    let Some(hit) = app.hits.get(position) else {
                        continue;
                    };
                    let record = hit.record.clone();
                    let ranges = app
                        .matcher
                        .as_ref()
                        .map_or(Vec::new(), |matcher| matcher.name_match_ranges(&record));
                    let rect = Rect::from_min_size(
                        canvas.min + Vec2::new(col as f32 * col_w, row as f32 * row_h),
                        Vec2::new(col_w - 5.0, row_h - 1.0),
                    );
                    draw_list_row(app, ui, &record, rect, &ranges, &mut open_path);
                }
            }
            if selection_changed {
                scroll_selection_into_view(app, ui, row_h, rows, col_w);
            }
        });
    if let Some(path) = open_path {
        perform_file_action(app, FileAction::Open(PathBuf::from(path)));
    }
}

fn draw_list_row(
    app: &mut NeutraApp,
    ui: &mut Ui,
    record: &neutra_core::FileRecord,
    rect: Rect,
    ranges: &[std::ops::Range<usize>],
    open_path: &mut Option<String>,
) {
    super::script_fonts::ensure(ui.ctx(), &record.path);
    let path = record.path.to_string();
    let response = ui.interact(rect, Id::new(("list-row", &path)), Sense::click_and_drag());
    if let Some(action) =
        super::file_interactions::interact(ui, &response, &path, record.kind == FileKind::Dir)
    {
        app.file_operations.dispatch(action);
    }
    let selected = row_info(app, ui, &response, record);
    ui.painter().rect_filled(
        rect,
        0.0,
        if selected {
            ACTIVE
        } else if response.hovered() {
            HOVER
        } else {
            CANVAS
        },
    );
    if selected || response.hovered() {
        ui.painter().rect_stroke(
            rect,
            0.0,
            Stroke::new(1.0_f32, if selected { ACID_STRONG } else { LINE_STRONG }),
            StrokeKind::Inside,
        );
    }
    let badge = Rect::from_center_size(
        rect.left_center() + Vec2::new(17.0, 0.0),
        Vec2::new(21.0, 18.0),
    );
    ui.painter().rect_stroke(
        badge,
        1.0,
        Stroke::new(1.0_f32, type_color(record)),
        StrokeKind::Inside,
    );
    ui.painter().text(
        badge.center(),
        Align2::CENTER_CENTER,
        type_badge(record),
        mono(7.0),
        TEXT,
    );
    paint_highlighted(
        ui,
        egui::pos2(badge.right() + 6.0, rect.center().y),
        Rect::from_min_max(egui::pos2(badge.right() + 6.0, rect.top()), rect.max),
        record.name(),
        ranges,
        &NameStyle {
            font: sans(SMALL),
            color: TEXT,
            highlight: ACID,
        },
    );
    if response.clicked() {
        app.select_result(&path, ui.input(|input| input.modifiers));
        selection::focus_clicked(app, &response);
    }
    if response.double_clicked() {
        *open_path = Some(path.clone());
    }
    result_context_menu(app, &response, &path);
}

pub(in crate::ui) fn grid_view(app: &mut NeutraApp, ui: &mut Ui) {
    if app.hits.is_empty() {
        empty_results(app, ui);
        return;
    }
    let tile_w = 108.0;
    let tile_h = 112.0;
    let columns = ((ui.available_width() - 14.0) / tile_w).floor().max(1.0) as usize;
    let rows = app.hits.len().div_ceil(columns);
    let mut open_path = None;
    let selection_changed = keyboard_selection(app, ui, &mut open_path);
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(8.0);
            let (canvas, _) = ui.allocate_exact_size(
                Vec2::new(ui.available_width(), rows as f32 * tile_h),
                Sense::hover(),
            );
            // Row culling: same rationale as list_view.
            let clip = ui.clip_rect().intersect(canvas);
            if clip.width() <= 0.0 || clip.height() <= 0.0 {
                return;
            }
            let first_row = (((clip.top() - canvas.top()) / tile_h).floor().max(0.0)) as usize;
            let last_row =
                (((clip.bottom() - canvas.top()) / tile_h).ceil().max(1.0) as usize).min(rows);
            for row in first_row..last_row {
                for col in 0..columns {
                    let position = row * columns + col;
                    let Some(hit) = app.hits.get(position) else {
                        continue;
                    };
                    let record = hit.record.clone();
                    let rect = Rect::from_min_size(
                        canvas.min + Vec2::new(7.0 + col as f32 * tile_w, row as f32 * tile_h),
                        Vec2::new(tile_w - 7.0, tile_h - 7.0),
                    );
                    draw_grid_tile(app, ui, &record, rect, &mut open_path);
                }
            }
            if selection_changed {
                scroll_selection_into_view(app, ui, tile_h, rows, ui.available_width());
            }
        });
    if let Some(path) = open_path {
        perform_file_action(app, FileAction::Open(PathBuf::from(path)));
    }
}

fn draw_grid_tile(
    app: &mut NeutraApp,
    ui: &mut Ui,
    record: &neutra_core::FileRecord,
    rect: Rect,
    open_path: &mut Option<String>,
) {
    super::script_fonts::ensure(ui.ctx(), &record.path);
    let path = record.path.to_string();
    let response = ui.interact(rect, Id::new(("grid-item", &path)), Sense::click_and_drag());
    if let Some(action) =
        super::file_interactions::interact(ui, &response, &path, record.kind == FileKind::Dir)
    {
        app.file_operations.dispatch(action);
    }
    let selected = row_info(app, ui, &response, record);
    ui.painter().rect_filled(
        rect,
        0.0,
        if selected {
            ACTIVE
        } else if response.hovered() {
            HOVER
        } else {
            CANVAS
        },
    );
    if selected || response.hovered() {
        ui.painter().rect_stroke(
            rect,
            0.0,
            Stroke::new(1.0_f32, if selected { ACID_STRONG } else { LINE_STRONG }),
            StrokeKind::Inside,
        );
    }
    paint_large_file_icon(ui, rect.center_top() + Vec2::new(0.0, 31.0), record);
    ui.painter()
        .with_clip_rect(Rect::from_min_max(
            rect.left_top() + Vec2::new(5.0, 58.0),
            rect.right_bottom() - Vec2::new(5.0, 16.0),
        ))
        .text(
            rect.center_top() + Vec2::new(0.0, 62.0),
            Align2::CENTER_TOP,
            shorten(record.name(), 28),
            sans(CAPTION),
            TEXT,
        );
    ui.painter().text(
        rect.center_bottom() - Vec2::new(0.0, 6.0),
        Align2::CENTER_BOTTOM,
        format_size(record.disk_bytes()),
        mono(CAPTION),
        MUTED,
    );
    if response.clicked() {
        app.select_result(&path, ui.input(|input| input.modifiers));
        selection::focus_clicked(app, &response);
    }
    if response.double_clicked() {
        *open_path = Some(path.clone());
    }
    result_context_menu(app, &response, &path);
}

fn paint_large_file_icon(ui: &Ui, center: egui::Pos2, record: &neutra_core::FileRecord) {
    let rect = Rect::from_center_size(center, Vec2::new(39.0, 48.0));
    let color = type_color(record);
    ui.painter().rect_filled(rect, 0.0, RAISED);
    ui.painter()
        .rect_stroke(rect, 0.0, Stroke::new(1.0_f32, color), StrokeKind::Inside);
    let fold = Rect::from_min_size(
        rect.right_top() - Vec2::new(10.0, 0.0),
        Vec2::new(10.0, 10.0),
    );
    ui.painter().line_segment(
        [fold.left_bottom(), fold.right_bottom()],
        Stroke::new(1.0_f32, LINE_STRONG),
    );
    ui.painter().line_segment(
        [fold.left_bottom(), fold.left_top()],
        Stroke::new(1.0_f32, LINE_STRONG),
    );
    ui.painter().text(
        rect.center_bottom() - Vec2::new(0.0, 7.0),
        Align2::CENTER_BOTTOM,
        type_badge(record),
        mono(8.0),
        TEXT,
    );
}
