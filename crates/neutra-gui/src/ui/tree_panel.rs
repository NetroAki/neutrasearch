//! Left side of the folder view: an expandable details tree with share, size
//! and item columns. A click opens a folder in the map, a double-click or the
//! chevron expands it, and the arrow keys walk it.

use super::hierarchy::{Hierarchy, TreeFile};
use super::treemap::TreeAction;
use super::*;
use std::cell::RefCell;
use std::collections::BTreeSet;

mod layout;
use layout::{row_layout, RowLayout};

const ROW_H: f32 = 26.0;
const MAX_FILE_ROWS: usize = Hierarchy::MAX_FILE_ROWS;

mod row_model;
use row_model::{flatten, short_count, KeyAction, Kind, Row};
mod keyboard;
use keyboard::keyboard;
mod row_paint;
use row_paint::paint_row;
mod chrome;
use chrome::{chevron, folder_glyph, header};

pub(super) fn tree_panel(
    ui: &mut Ui,
    hierarchy: &Hierarchy,
    current: &str,
    expanded: &mut BTreeSet<String>,
    action: &RefCell<Option<TreeAction>>,
) {
    ui.painter().rect_filled(ui.max_rect(), 0.0, SURFACE);
    header(ui);
    let rows = flatten(hierarchy, current, expanded);
    let mut scroll_to = None;
    if let Some(key_action) = keyboard(ui, &rows, current, expanded) {
        match key_action {
            KeyAction::Focus(path, row) => {
                let id = Id::new(("tree-row", path.as_str()));
                ui.ctx().memory_mut(|memory| memory.request_focus(id));
                scroll_to = Some((row as f32 * ROW_H - ROW_H * 3.0).max(0.0));
                *action.borrow_mut() = Some(TreeAction::Focus(path, id));
            }
            KeyAction::Expand(path) => {
                expanded.insert(path);
            }
            KeyAction::Collapse(path) => {
                expanded.remove(&path);
            }
            KeyAction::Navigate(path) => *action.borrow_mut() = Some(TreeAction::Navigate(path)),
            KeyAction::Open(path) => *action.borrow_mut() = Some(TreeAction::Open(path)),
        }
    }
    let mut area = egui::ScrollArea::vertical().auto_shrink([false, false]);
    if let Some(offset) = scroll_to {
        area = area.vertical_scroll_offset(offset);
    }
    area.show_rows(ui, ROW_H, rows.len(), |ui, range| {
        for row in &rows[range] {
            paint_row(ui, hierarchy, row, current, expanded, action);
        }
    });
}

#[cfg(test)]
mod tests;
