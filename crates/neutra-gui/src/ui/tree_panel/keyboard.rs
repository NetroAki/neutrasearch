use super::*;

/// Handle keys for the focused tree item, using the visible rows as the
/// navigation order. Files participate in focus movement just like folders.
pub(super) fn keyboard(
    ui: &Ui,
    rows: &[Row],
    current: &str,
    expanded: &mut BTreeSet<String>,
) -> Option<KeyAction> {
    let focused = ui.memory(|memory| memory.focused());
    let focused_row = rows
        .iter()
        .position(|row| focused == Some(Id::new(("tree-row", row.path))));
    if focused_row.is_none() && ui.ctx().egui_wants_keyboard_input() {
        return None;
    }
    let pressed = |key| ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, key));
    let at = focused_row.or_else(|| {
        rows.iter()
            .position(|row| matches!(row.kind, Kind::Folder { .. }) && row.path == current)
    })?;
    let row = &rows[at];
    let folder = matches!(row.kind, Kind::Folder { .. });
    let interactive =
        |index: usize| matches!(rows[index].kind, Kind::Folder { .. } | Kind::File(_));
    if pressed(egui::Key::ArrowDown) {
        let target = (at + 1..rows.len()).find(|&index| interactive(index))?;
        return Some(KeyAction::Focus(rows[target].path.to_owned(), target));
    }
    if pressed(egui::Key::ArrowUp) {
        let target = (0..at).rev().find(|&index| interactive(index))?;
        return Some(KeyAction::Focus(rows[target].path.to_owned(), target));
    }
    if pressed(egui::Key::ArrowRight) && folder {
        if expanded.insert(row.path.to_owned()) {
            return Some(KeyAction::Expand(row.path.to_owned()));
        }
        // Only the contiguous block below the focused folder belongs to
        // it; once depth returns to the folder's level (or above), the
        // rows belong to siblings or ancestors and must not be entered.
        let child = (at + 1..rows.len())
            .take_while(|&index| rows[index].depth > row.depth)
            .find(|&index| interactive(index));
        return child.map(|index| KeyAction::Focus(rows[index].path.to_owned(), index));
    }
    if pressed(egui::Key::ArrowLeft) {
        if folder && expanded.remove(row.path) {
            return Some(KeyAction::Collapse(row.path.to_owned()));
        }
        let parent = (0..at).rev().find(|&index| {
            matches!(rows[index].kind, Kind::Folder { .. }) && rows[index].depth < row.depth
        });
        return parent.map(|index| KeyAction::Focus(rows[index].path.to_owned(), index));
    }
    if pressed(egui::Key::Enter) {
        return match row.kind {
            Kind::Folder { .. } => Some(KeyAction::Navigate(row.path.to_owned())),
            Kind::File(file) => Some(KeyAction::Open(file.path.clone())),
            _ => None,
        };
    }
    None
}
