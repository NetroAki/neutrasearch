use super::*;

/// Apply arrow/Enter selection and indicate when scrolling is needed.
pub(super) fn keyboard_selection(
    app: &mut NeutraApp,
    ui: &Ui,
    open_path: &mut Option<String>,
) -> bool {
    if app.hits.is_empty() || !super::file_shortcuts::allowed(app, ui) {
        return false;
    }
    let selected_position = app.selected.as_ref().and_then(|path| {
        app.hits
            .iter()
            .position(|hit| hit.record.path.as_ref() == path)
    });
    let modifiers = ui.input(|input| input.modifiers);
    let navigation = egui::Modifiers {
        shift: modifiers.shift,
        ..egui::Modifiers::NONE
    };
    let down = ui.input_mut(|input| input.consume_key(navigation, egui::Key::ArrowDown));
    let up = ui.input_mut(|input| input.consume_key(navigation, egui::Key::ArrowUp));
    let mut changed = false;
    if down || up {
        let next = match (selected_position, down) {
            (Some(position), true) => (position + 1).min(app.hits.len() - 1),
            (Some(position), false) => position.saturating_sub(1),
            (None, true) => 0,
            (None, false) => app.hits.len() - 1,
        };
        let path = app.hits[next].record.path.to_string();
        app.select_result(&path, navigation);
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
pub(super) fn scroll_selection_into_view(
    app: &NeutraApp,
    ui: &Ui,
    row_h: f32,
    rows: usize,
    col_w: f32,
) {
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
