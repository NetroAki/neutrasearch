use super::*;

// Filtering happens entirely in the query engine (terms, regex, case, and
// field scope are all `Query` options now). The views draw `app.hits` as the
// engine ranked it — no client-side re-filtering, which previously broke the
// `ext:`/`kind:`/`size:`/`under:` syntax and duplicated engine work per frame.

pub(super) fn details_view(app: &mut NeutraApp, ui: &mut Ui) {
    if app.hits.is_empty() {
        empty_results(app, ui);
        return;
    }
    details_header(app, ui);
    let row_h = 29.0;
    let mut open_path = None;
    // Handle selection keys before the scroll area so a new selection can be
    // scrolled into view in the same frame.
    let selection_changed = keyboard_selection(app, ui, &mut open_path);
    let selected_position = selection_changed
        .then(|| {
            app.selected.as_ref().and_then(|path| {
                app.hits
                    .iter()
                    .position(|hit| hit.record.path.as_ref() == path)
            })
        })
        .flatten();
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show_rows(ui, row_h, app.hits.len(), |ui, range| {
            if let Some(position) = selected_position {
                if !range.contains(&position) {
                    let top = ui.min_rect().top() + position as f32 * row_h;
                    ui.scroll_to_rect(
                        Rect::from_min_size(
                            egui::pos2(ui.min_rect().left(), top),
                            Vec2::new(ui.available_width(), row_h),
                        ),
                        None,
                    );
                }
            }
            for visible_row in range {
                let record = &app.hits[visible_row].record;
                let ranges = app
                    .matcher
                    .as_ref()
                    .map_or(Vec::new(), |matcher| matcher.name_match_ranges(record));
                let path = record.path.to_string();
                let (rect, response) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), Sense::click());
                let selected = app.selected.as_deref() == Some(record.path.as_ref());
                paint_details_row(ui, rect, record, visible_row, selected, &ranges);
                if response.clicked() {
                    app.selected = Some(path.clone());
                    surrender_widget_focus(ui);
                }
                if response.double_clicked() {
                    open_path = Some(path.clone());
                }
                result_context_menu(&response, ui, &path, &mut open_path);
            }
        });
    if let Some(path) = open_path {
        perform_file_action(app, FileAction::Open(PathBuf::from(path)));
    }
}

fn details_header(app: &mut NeutraApp, ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 25.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, RAISED);
    ui.painter().hline(
        rect.x_range(),
        rect.bottom(),
        Stroke::new(1.0_f32, LINE_STRONG),
    );
    let columns = detail_columns(rect);
    for x in [
        columns.name.max.x,
        columns.path.max.x,
        columns.modified.max.x,
        columns.size.max.x,
    ] {
        ui.painter()
            .vline(x, rect.y_range(), Stroke::new(1.0_f32, LINE));
    }
    sort_header(app, ui, columns.name, SortMode::Name, "Name", false);
    sort_header(app, ui, columns.path, SortMode::Path, "Path", false);
    sort_header(
        app,
        ui,
        columns.modified,
        SortMode::Modified,
        "Modified",
        false,
    );
    sort_header(app, ui, columns.size, SortMode::Size, "Size", true);
}

fn sort_header(
    app: &mut NeutraApp,
    ui: &mut Ui,
    rect: Rect,
    mode: SortMode,
    label: &str,
    align_right: bool,
) {
    let hit_rect = rect.shrink2(Vec2::new(5.0, 0.0));
    let response = ui
        .interact(hit_rect, Id::new(("sort", label)), Sense::click())
        .on_hover_text(format!("Sort by {label}"));
    let active = app.sort_mode == mode;
    let arrow = if active {
        let reversed = app.sort_reversed;
        match mode {
            SortMode::Name | SortMode::Path => {
                if reversed { "  ↓" } else { "  ↑" }
            }
            SortMode::Modified | SortMode::Size => {
                if reversed { "  ↑" } else { "  ↓" }
            }
            SortMode::Relevance => "",
        }
    } else {
        ""
    };
    let (anchor, alignment) = if align_right {
        (hit_rect.right_center(), Align2::RIGHT_CENTER)
    } else {
        (hit_rect.left_center(), Align2::LEFT_CENTER)
    };
    ui.painter().text(
        anchor,
        alignment,
        format!("{label}{arrow}"),
        sans(10.0),
        if active || response.hovered() {
            ACID
        } else {
            MUTED
        },
    );
    if response.clicked() {
        if active {
            // Clicking the active column flips its direction.
            app.sort_reversed = !app.sort_reversed;
        } else {
            app.sort_mode = mode;
            app.sort_reversed = false;
        }
        app.save_settings();
        app.requery();
    }
}

struct DetailColumns {
    name: Rect,
    path: Rect,
    modified: Rect,
    size: Rect,
}

fn detail_columns(rect: Rect) -> DetailColumns {
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
struct NameStyle {
    font: FontId,
    color: Color32,
    highlight: Color32,
}

fn paint_highlighted(
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

fn paint_details_row(
    ui: &mut Ui,
    rect: Rect,
    record: &neutra_core::FileRecord,
    row: usize,
    selected: bool,
    ranges: &[std::ops::Range<usize>],
) {
    let hovered = ui.rect_contains_pointer(rect);
    let fill = if selected {
        ACTIVE
    } else if hovered {
        HOVER
    } else if row.is_multiple_of(2) {
        CANVAS
    } else {
        Color32::from_rgb(25, 26, 36)
    };
    ui.painter().rect_filled(rect, 0.0, fill);
    ui.painter().hline(
        rect.x_range(),
        rect.bottom(),
        Stroke::new(1.0_f32, Color32::from_rgb(39, 41, 54)),
    );
    let columns = detail_columns(rect);
    let badge = Rect::from_center_size(
        columns.name.left_center() + Vec2::new(21.0, 0.0),
        Vec2::new(22.0, 19.0),
    );
    let badge_color = type_color(record);
    ui.painter().rect_filled(badge, 1.0, RAISED);
    ui.painter().rect_stroke(
        badge,
        1.0,
        Stroke::new(1.0_f32, badge_color),
        StrokeKind::Inside,
    );
    ui.painter().text(
        badge.center(),
        Align2::CENTER_CENTER,
        type_badge(record),
        mono(7.5),
        badge_color,
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
            font: sans(11.5),
            color: TEXT,
            highlight: ACID_STRONG,
        },
    );
    let metadata = if selected {
        Color32::from_rgb(207, 209, 222)
    } else {
        MUTED
    };
    ui.painter()
        .with_clip_rect(columns.path.shrink2(Vec2::new(7.0, 0.0)))
        .text(
            columns.path.left_center() + Vec2::new(7.0, 0.0),
            Align2::LEFT_CENTER,
            parent_path(&record.path),
            mono(9.5),
            metadata,
        );
    ui.painter().text(
        columns.modified.left_center() + Vec2::new(7.0, 0.0),
        Align2::LEFT_CENTER,
        format_mtime(record.mtime),
        sans(10.0),
        metadata,
    );
    ui.painter().text(
        columns.size.right_center() - Vec2::new(7.0, 0.0),
        Align2::RIGHT_CENTER,
        format_size(record.size),
        mono(9.5),
        metadata,
    );
}

pub(super) fn list_view(app: &mut NeutraApp, ui: &mut Ui) {
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
            let last_col = (((clip.right() - canvas.left()) / col_w).ceil().max(1.0) as usize)
                .min(columns);
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
    let path = record.path.to_string();
    let response = ui.interact(rect, Id::new(("list-row", &path)), Sense::click());
    let selected = app.selected.as_deref() == Some(record.path.as_ref());
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
    ui.painter()
        .rect_stroke(badge, 1.0, Stroke::new(1.0_f32, type_color(record)), StrokeKind::Inside);
    ui.painter().text(
        badge.center(),
        Align2::CENTER_CENTER,
        type_badge(record),
        mono(7.0),
        type_color(record),
    );
    paint_highlighted(
        ui,
        egui::pos2(badge.right() + 6.0, rect.center().y),
        Rect::from_min_max(
            egui::pos2(badge.right() + 6.0, rect.top()),
            rect.max,
        ),
        record.name(),
        ranges,
        &NameStyle {
            font: sans(10.5),
            color: TEXT,
            highlight: ACID_STRONG,
        },
    );
    if response.clicked() {
        app.selected = Some(path.clone());
        surrender_widget_focus(ui);
    }
    if response.double_clicked() {
        *open_path = Some(path.clone());
    }
    result_context_menu(&response, ui, &path, open_path);
}

pub(super) fn grid_view(app: &mut NeutraApp, ui: &mut Ui) {
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
    let path = record.path.to_string();
    let response = ui.interact(rect, Id::new(("grid-item", &path)), Sense::click());
    let selected = app.selected.as_deref() == Some(record.path.as_ref());
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
            sans(10.0),
            TEXT,
        );
    ui.painter().text(
        rect.center_bottom() - Vec2::new(0.0, 6.0),
        Align2::CENTER_BOTTOM,
        format_size(record.size),
        mono(8.5),
        MUTED,
    );
    if response.clicked() {
        app.selected = Some(path.clone());
        surrender_widget_focus(ui);
    }
    if response.double_clicked() {
        *open_path = Some(path.clone());
    }
    result_context_menu(&response, ui, &path, open_path);
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
        color,
    );
}

fn empty_results(app: &mut NeutraApp, ui: &mut Ui) {
    let invalid_regex = app.regex_mode
        && !app.query.is_empty()
        && RegexBuilder::new(&app.query)
            .case_insensitive(!app.case_sensitive)
            .build()
            .is_err();
    ui.centered_and_justified(|ui| {
        ui.vertical_centered(|ui| {
            paint_search_icon(ui, if invalid_regex { ERROR } else { SUBTLE });
            ui.add_space(7.0);
            ui.label(
                RichText::new(if invalid_regex {
                    "Invalid regular expression"
                } else {
                    "No matching objects"
                })
                .font(sans(15.0))
                .strong(),
            );
            ui.label(
                RichText::new(if invalid_regex {
                    "Edit the expression or reset search options."
                } else if app.regex_mode {
                    "No files match this regular expression."
                } else {
                    "Try fewer words or search a different location."
                })
                .font(sans(11.0))
                .color(MUTED),
            );
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let action_width = if app.query.is_empty() { 132.0 } else { 222.0 };
                ui.add_space((ui.available_width() - action_width).max(0.0) * 0.5);
                if !app.query.is_empty() && secondary_button(ui, "Clear search", MUTED).clicked() {
                    app.query.clear();
                    app.requery();
                }
                let has_options = app.regex_mode
                    || app.case_sensitive
                    || app.whole_word
                    || app.ignore_accents
                    || app.search_mode != SearchMode::Name
                    || app.kind_filter != KindFilter::All
                    || app.scope_root.is_some();
                if has_options && secondary_button(ui, "Reset search options", MUTED).clicked() {
                    app.regex_mode = false;
                    app.case_sensitive = false;
                    app.whole_word = false;
                    app.ignore_accents = false;
                    app.search_mode = SearchMode::Name;
                    app.kind_filter = KindFilter::All;
                    app.scope_root = None;
                    app.save_settings();
                    app.requery();
                }
            });
        });
    });
}

pub(super) fn surrender_widget_focus(ui: &Ui) {
    ui.memory_mut(|memory| {
        if let Some(focused) = memory.focused() {
            memory.surrender_focus(focused);
        }
    });
}

/// Apply arrow/Enter selection. Returns true when the selection moved this
/// frame so the views can scroll it into view.
fn keyboard_selection(
    app: &mut NeutraApp,
    ui: &Ui,
    open_path: &mut Option<String>,
) -> bool {
    if app.hits.is_empty() {
        return false;
    }
    let selected_position = app.selected.as_ref().and_then(|path| {
        app.hits
            .iter()
            .position(|hit| hit.record.path.as_ref() == path)
    });
    let down = ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown));
    let up = ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp));
    let mut changed = false;
    if down || up {
        let position = selected_position.unwrap_or(if down { 0 } else { app.hits.len() - 1 });
        let next = if down {
            (position + 1).min(app.hits.len() - 1)
        } else {
            position.saturating_sub(1)
        };
        app.selected = Some(app.hits[next].record.path.to_string());
        changed = true;
    }
    if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
        if let Some(path) = &app.selected {
            *open_path = Some(path.clone());
        }
    }
    changed
}

/// Best-effort viewport nudge toward the selected row after a keyboard move
/// in the grid/list layouts (details view scrolls by row index directly).
fn scroll_selection_into_view(app: &NeutraApp, ui: &Ui, row_h: f32, rows: usize, col_w: f32) {
    let Some(selected) = app.selected.as_ref() else {
        return;
    };
    let Some(position) = app
        .hits
        .iter()
        .position(|hit| hit.record.path.as_ref() == selected)
    else {
        return;
    };
    let rows = rows.max(1);
    let (col, row) = (position / rows, position % rows);
    let top = ui.min_rect().top() + row as f32 * row_h;
    let left = ui.min_rect().left() + col as f32 * col_w;
    ui.scroll_to_rect(
        Rect::from_min_size(egui::pos2(left, top), Vec2::new(col_w, row_h)),
        None,
    );
}

fn result_context_menu(
    response: &egui::Response,
    ui: &Ui,
    path: &str,
    open_path: &mut Option<String>,
) {
    response.context_menu(|menu| {
        if menu.button("Open").clicked() {
            *open_path = Some(path.to_owned());
            menu.close();
        }
        if menu.button("Reveal in file manager").clicked() {
            let _ = launch_file_action(FileAction::Reveal(PathBuf::from(path)));
            menu.close();
        }
        if menu.button("Copy full path    Ctrl+Insert").clicked() {
            copy_to_clipboard(ui, path);
            menu.close();
        }
    });
}

pub(super) fn perform_file_action(app: &mut NeutraApp, action: FileAction) {
    let description = match &action {
        FileAction::Open(path) => format!("open {}", path.display()),
        FileAction::Reveal(path) => format!("reveal {}", path.display()),
    };
    let result = launch_file_action(action);
    app.lanes.insert(
        "file-action".into(),
        LaneState {
            label: "FILE ACTION".into(),
            status: match &result {
                Ok(()) => description,
                Err(error) => format!("{description}: {error}"),
            },
            error: result.is_err(),
            ..Default::default()
        },
    );
}
