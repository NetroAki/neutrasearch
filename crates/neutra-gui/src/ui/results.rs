use super::*;

pub(super) fn visible_indices(app: &NeutraApp) -> Vec<usize> {
    let query = app.query.trim();
    let regex = if app.regex_mode && !query.is_empty() {
        RegexBuilder::new(query)
            .case_insensitive(!app.case_sensitive)
            .build()
            .ok()
    } else {
        None
    };
    app.hits
        .iter()
        .enumerate()
        .filter_map(|(index, hit)| {
            if query.is_empty() {
                return Some(index);
            }
            let record = &hit.record;
            let candidate = match app.search_mode {
                SearchMode::Name => record.name().to_owned(),
                SearchMode::NameAndPath => format!("{} {}", record.name(), record.path),
                SearchMode::Path => record.path.to_string(),
            };
            let matches = if app.regex_mode {
                regex
                    .as_ref()
                    .is_some_and(|regex| regex.is_match(&candidate))
            } else if app.case_sensitive {
                candidate.contains(query)
            } else {
                candidate.to_lowercase().contains(&query.to_lowercase())
            };
            matches.then_some(index)
        })
        .collect()
}

pub(super) fn details_view(app: &mut NeutraApp, ui: &mut Ui) {
    let indices = visible_indices(app);
    if indices.is_empty() {
        empty_results(app, ui);
        return;
    }
    details_header(app, ui);
    let row_h = 29.0;
    let mut open_path = None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show_rows(ui, row_h, indices.len(), |ui, range| {
            for visible_row in range {
                let hit_index = indices[visible_row];
                let record = &app.hits[hit_index].record;
                let path = record.path.to_string();
                let (rect, response) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), Sense::click());
                let selected = app.selected.as_deref() == Some(record.path.as_ref());
                paint_details_row(ui, rect, record, visible_row, selected);
                if response.clicked() {
                    app.selected = Some(path.clone());
                }
                if response.double_clicked() {
                    open_path = Some(path.clone());
                }
                result_context_menu(&response, ui, &path, &mut open_path);
            }
        });
    keyboard_selection(app, &indices, ui, &mut open_path);
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
    ui.painter().text(
        columns.name.left_center() + Vec2::new(9.0, 0.0),
        Align2::LEFT_CENTER,
        "Name",
        sans(10.0),
        MUTED,
    );
    ui.painter().text(
        columns.path.left_center() + Vec2::new(7.0, 0.0),
        Align2::LEFT_CENTER,
        "Path",
        sans(10.0),
        MUTED,
    );
    let sort_rect = columns.modified.shrink2(Vec2::new(6.0, 0.0));
    let response = ui.interact(sort_rect, Id::new("modified-sort"), Sense::click());
    ui.painter().text(
        sort_rect.left_center(),
        Align2::LEFT_CENTER,
        format!("{}  v", app.sort_mode.label()),
        sans(10.0),
        MUTED,
    );
    if response.clicked() {
        app.sort_mode = match app.sort_mode {
            SortMode::Modified => SortMode::Name,
            SortMode::Name => SortMode::Size,
            SortMode::Size => SortMode::Path,
            SortMode::Path => SortMode::Modified,
        };
        app.requery();
    }
    ui.painter().text(
        columns.size.right_center() - Vec2::new(7.0, 0.0),
        Align2::RIGHT_CENTER,
        "Size",
        sans(10.0),
        MUTED,
    );
}

struct DetailColumns {
    name: Rect,
    path: Rect,
    modified: Rect,
    size: Rect,
    action: Rect,
}

fn detail_columns(rect: Rect) -> DetailColumns {
    let action_w = 30.0;
    let content = rect.width() - action_w;
    let name_w = content * 0.32;
    let path_w = content * 0.39;
    let modified_w = content * 0.17;
    let size_w = content - name_w - path_w - modified_w;
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
    x += size_w;
    let action = Rect::from_min_size(
        egui::pos2(x, rect.top()),
        Vec2::new(action_w, rect.height()),
    );
    DetailColumns {
        name,
        path,
        modified,
        size,
        action,
    }
}

fn paint_details_row(
    ui: &mut Ui,
    rect: Rect,
    record: &neutra_core::FileRecord,
    row: usize,
    selected: bool,
) {
    let hovered = ui.rect_contains_pointer(rect);
    let fill = if selected {
        ACTIVE
    } else if hovered {
        HOVER
    } else if row.is_multiple_of(2) {
        CANVAS
    } else {
        Color32::from_rgb(23, 26, 28)
    };
    ui.painter().rect_filled(rect, 0.0, fill);
    ui.painter().hline(
        rect.x_range(),
        rect.bottom(),
        Stroke::new(1.0_f32, Color32::from_rgb(35, 39, 41)),
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
    ui.painter()
        .with_clip_rect(Rect::from_min_max(
            egui::pos2(badge.right() + 6.0, columns.name.top()),
            columns.name.max,
        ))
        .text(
            egui::pos2(badge.right() + 6.0, columns.name.center().y),
            Align2::LEFT_CENTER,
            record.name(),
            sans(11.5),
            TEXT,
        );
    let metadata = if selected {
        Color32::from_rgb(205, 220, 215)
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
    if hovered || selected {
        for offset in [-4.0, 0.0, 4.0] {
            ui.painter().circle_filled(
                columns.action.center() + Vec2::new(offset, 0.0),
                1.2,
                MUTED,
            );
        }
    }
}

pub(super) fn list_view(app: &mut NeutraApp, ui: &mut Ui) {
    let indices = visible_indices(app);
    if indices.is_empty() {
        empty_results(app, ui);
        return;
    }
    let row_h = 28.0;
    let rows = ((ui.available_height() - 8.0) / row_h).floor().max(1.0) as usize;
    let columns = indices.len().div_ceil(rows);
    let col_w = 240.0;
    let mut open_path = None;
    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let (canvas, _) = ui.allocate_exact_size(
                Vec2::new(columns as f32 * col_w, rows as f32 * row_h),
                Sense::hover(),
            );
            for (position, hit_index) in indices.iter().copied().enumerate() {
                let col = position / rows;
                let row = position % rows;
                let rect = Rect::from_min_size(
                    canvas.min + Vec2::new(col as f32 * col_w, row as f32 * row_h),
                    Vec2::new(col_w - 5.0, row_h - 1.0),
                );
                let record = &app.hits[hit_index].record;
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
                    type_color(record),
                );
                ui.painter()
                    .with_clip_rect(Rect::from_min_max(
                        egui::pos2(badge.right() + 6.0, rect.top()),
                        rect.max,
                    ))
                    .text(
                        egui::pos2(badge.right() + 6.0, rect.center().y),
                        Align2::LEFT_CENTER,
                        record.name(),
                        sans(10.5),
                        TEXT,
                    );
                if response.clicked() {
                    app.selected = Some(path.clone());
                }
                if response.double_clicked() {
                    open_path = Some(path.clone());
                }
                result_context_menu(&response, ui, &path, &mut open_path);
            }
        });
    keyboard_selection(app, &indices, ui, &mut open_path);
    if let Some(path) = open_path {
        perform_file_action(app, FileAction::Open(PathBuf::from(path)));
    }
}

pub(super) fn grid_view(app: &mut NeutraApp, ui: &mut Ui) {
    let indices = visible_indices(app);
    if indices.is_empty() {
        empty_results(app, ui);
        return;
    }
    let tile_w = 108.0;
    let tile_h = 112.0;
    let columns = ((ui.available_width() - 14.0) / tile_w).floor().max(1.0) as usize;
    let rows = indices.len().div_ceil(columns);
    let mut open_path = None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(8.0);
            let (canvas, _) = ui.allocate_exact_size(
                Vec2::new(ui.available_width(), rows as f32 * tile_h),
                Sense::hover(),
            );
            for (position, hit_index) in indices.iter().copied().enumerate() {
                let col = position % columns;
                let row = position / columns;
                let rect = Rect::from_min_size(
                    canvas.min + Vec2::new(7.0 + col as f32 * tile_w, row as f32 * tile_h),
                    Vec2::new(tile_w - 7.0, tile_h - 7.0),
                );
                let record = &app.hits[hit_index].record;
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
                }
                if response.double_clicked() {
                    open_path = Some(path.clone());
                }
                result_context_menu(&response, ui, &path, &mut open_path);
            }
        });
    keyboard_selection(app, &indices, ui, &mut open_path);
    if let Some(path) = open_path {
        perform_file_action(app, FileAction::Open(PathBuf::from(path)));
    }
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

fn empty_results(app: &NeutraApp, ui: &mut Ui) {
    ui.centered_and_justified(|ui| {
        ui.vertical_centered(|ui| {
            paint_search_icon(ui, SUBTLE);
            ui.add_space(7.0);
            ui.label(
                RichText::new("No matching objects")
                    .font(sans(15.0))
                    .strong(),
            );
            ui.label(
                RichText::new(if app.regex_mode {
                    "Check the regular expression or change search mode."
                } else {
                    "Try fewer words or search a different location."
                })
                .font(sans(11.0))
                .color(MUTED),
            );
        });
    });
}

fn keyboard_selection(
    app: &mut NeutraApp,
    indices: &[usize],
    ui: &Ui,
    open_path: &mut Option<String>,
) {
    if indices.is_empty() {
        return;
    }
    let selected_position = app.selected.as_ref().and_then(|path| {
        indices
            .iter()
            .position(|index| app.hits[*index].record.path.as_ref() == path)
    });
    let down = ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown));
    let up = ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp));
    if down || up {
        let position = selected_position.unwrap_or(if down { 0 } else { indices.len() - 1 });
        let next = if down {
            (position + 1).min(indices.len() - 1)
        } else {
            position.saturating_sub(1)
        };
        app.selected = Some(app.hits[indices[next]].record.path.to_string());
    }
    if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
        if let Some(path) = &app.selected {
            *open_path = Some(path.clone());
        }
    }
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
        if menu.button("Copy path").clicked() {
            ui.ctx().copy_text(path.to_owned());
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
