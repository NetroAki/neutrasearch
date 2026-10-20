use super::*;
mod adapters;
mod empty;
mod keyboard;
mod selection;
mod view;
use adapters::{detail_columns, paint_details_row};
pub(super) use empty::show as empty_results;
use keyboard::{keyboard_selection, scroll_selection_into_view};
use selection::{result_context_menu, row_info};
pub(super) use view::{grid_view, list_view};

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
    let row_h = 26.0;
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
                let record = app.hits[visible_row].record.clone();
                super::script_fonts::ensure(ui.ctx(), &record.path);
                let ranges = app
                    .matcher
                    .as_ref()
                    .map_or(Vec::new(), |matcher| matcher.name_match_ranges(&record));
                let path = record.path.to_string();
                let (rect, _) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), Sense::hover());
                let response = ui.interact(
                    rect,
                    Id::new(("details-row", &path)),
                    Sense::click_and_drag(),
                );
                let selected = row_info(app, ui, &response, &record);
                paint_details_row(ui, rect, &record, selected, &ranges);
                if let Some(action) = super::file_interactions::interact(
                    ui,
                    &response,
                    &path,
                    record.kind == FileKind::Dir,
                ) {
                    app.file_operations.dispatch(action);
                }
                if response.clicked() {
                    app.select_result(&path, ui.input(|input| input.modifiers));
                    selection::focus_clicked(app, &response);
                }
                if response.double_clicked() {
                    open_path = Some(path.clone());
                }
                result_context_menu(app, &response, &path);
            }
        });
    if let Some(path) = open_path {
        perform_file_action(app, FileAction::Open(PathBuf::from(path)));
    }
}

fn details_header(app: &mut NeutraApp, ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 25.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, SURFACE);
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
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            true,
            if active {
                let ascending = match mode {
                    SortMode::Name | SortMode::Path => !app.sort_reversed,
                    SortMode::Modified | SortMode::Size | SortMode::Relevance => app.sort_reversed,
                };
                format!(
                    "Sort by {label}, {}",
                    if ascending { "ascending" } else { "descending" }
                )
            } else {
                format!("Sort by {label}")
            },
        )
    });
    let arrow = if active {
        let reversed = app.sort_reversed;
        match mode {
            SortMode::Name | SortMode::Path => {
                if reversed {
                    "  ↓"
                } else {
                    "  ↑"
                }
            }
            SortMode::Modified | SortMode::Size => {
                if reversed {
                    "  ↑"
                } else {
                    "  ↓"
                }
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
        format!("{}{arrow}", tracked(label)),
        sans(MICRO),
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

pub(super) fn surrender_widget_focus(ui: &Ui) {
    ui.memory_mut(|memory| {
        if let Some(focused) = memory.focused() {
            memory.surrender_focus(focused);
        }
    });
}

pub(super) fn perform_file_action(app: &mut NeutraApp, action: FileAction) {
    app.file_operations.dispatch(action);
}
