use super::*;

pub(super) fn move_result_selection(app: &mut NeutraApp, forward: bool) {
    if app.hits.is_empty() {
        app.selected = None;
        return;
    }
    let current = app.selected.as_ref().and_then(|path| {
        app.hits
            .iter()
            .position(|hit| hit.record.path.as_ref() == path)
    });
    let position = match (current, forward) {
        (Some(position), true) => (position + 1).min(app.hits.len() - 1),
        (Some(position), false) => position.saturating_sub(1),
        (None, true) => 0,
        (None, false) => app.hits.len() - 1,
    };
    let path = app.hits[position].record.path.to_string();
    app.select_single(&path);
}
