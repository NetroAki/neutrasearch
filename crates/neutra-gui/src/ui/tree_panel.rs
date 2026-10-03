//! Left side of the folder view: an expandable details tree with share, size
//! and item columns. A click opens a folder in the map, a double-click or the
//! chevron expands it, and the arrow keys walk it.

use super::hierarchy::{Hierarchy, TreeFile};
use super::treemap::TreeAction;
use super::*;
use std::cell::RefCell;
use std::collections::BTreeSet;

const ROW_H: f32 = 26.0;
const MAX_FILE_ROWS: usize = 200;
const ITEMS_W: f32 = 52.0;
const SIZE_W: f32 = 64.0;
const BAR_W: f32 = 34.0;

enum Kind<'a> {
    Folder { size: u64, count: u64 },
    File(&'a TreeFile),
    More(usize),
    Loading,
}

struct Row<'a> {
    path: &'a str,
    depth: usize,
    share: f32,
    kind: Kind<'a>,
}

enum Item<'a> {
    Dir { path: &'a str, depth: usize, size: u64, count: u64, parent: u64 },
    Files { path: &'a str, depth: usize },
}

fn share(part: u64, whole: u64) -> f32 {
    if whole == 0 { 0.0 } else { (part as f64 / whole as f64).min(1.0) as f32 }
}

fn short_count(count: u64) -> String {
    match count {
        0..=999 => count.to_string(),
        1_000..=999_999 => format!("{:.1}K", count as f64 / 1e3),
        1_000_000..=999_999_999 => format!("{:.1}M", count as f64 / 1e6),
        _ => format!("{:.1}B", count as f64 / 1e9),
    }
}

/// Visible rows in display order: each expanded folder lists its subfolders
/// and then, for the open folder only, its largest files.
fn flatten<'a>(hierarchy: &'a Hierarchy, current: &str, expanded: &BTreeSet<String>) -> Vec<Row<'a>> {
    let root = hierarchy.folders.get("/").cloned().unwrap_or_default();
    let mut rows = Vec::new();
    let mut stack = vec![Item::Dir { path: "/", depth: 0, size: root.size, count: root.count, parent: root.size }];
    while let Some(item) = stack.pop() {
        match item {
            Item::Dir { path, depth, size, count, parent } => {
                rows.push(Row { path, depth, share: share(size, parent), kind: Kind::Folder { size, count } });
                if !expanded.contains(path) {
                    continue;
                }
                let Some(folder) = hierarchy.folders.get(path) else {
                    rows.push(Row { path, depth: depth + 1, share: 0.0, kind: Kind::Loading });
                    continue;
                };
                if path == current && !folder.direct_files.is_empty() {
                    stack.push(Item::Files { path, depth: depth + 1 });
                }
                for child in folder.children.iter().rev() {
                    stack.push(Item::Dir {
                        path: child.path.as_str(),
                        depth: depth + 1,
                        size: child.size,
                        count: child.count,
                        parent: folder.size,
                    });
                }
            }
            Item::Files { path, depth } => {
                let Some(folder) = hierarchy.folders.get(path) else { continue };
                for file in folder.direct_files.iter().take(MAX_FILE_ROWS) {
                    rows.push(Row { path: file.path.as_str(), depth, share: share(file.size, folder.size), kind: Kind::File(file) });
                }
                let hidden = folder.direct_files.len().saturating_sub(MAX_FILE_ROWS);
                if hidden > 0 || folder.files_truncated {
                    rows.push(Row { path, depth, share: 0.0, kind: Kind::More(hidden) });
                }
            }
        }
    }
    rows
}

/// Arrow-key navigation. Returns the folder to open and the row to scroll to.
fn keyboard(ui: &Ui, rows: &[Row], current: &str, expanded: &mut BTreeSet<String>) -> Option<(String, usize)> {
    if ui.ctx().egui_wants_keyboard_input() {
        return None;
    }
    let pressed = |key| ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, key));
    let at = rows.iter().position(|row| matches!(row.kind, Kind::Folder { .. }) && row.path == current)?;
    let folder_after = |from: usize| (from + 1..rows.len()).find(|&i| matches!(rows[i].kind, Kind::Folder { .. }));
    let folder_before = |from: usize| (0..from).rev().find(|&i| matches!(rows[i].kind, Kind::Folder { .. }));
    let target = if pressed(egui::Key::ArrowDown) {
        folder_after(at)
    } else if pressed(egui::Key::ArrowUp) {
        folder_before(at)
    } else if pressed(egui::Key::ArrowRight) {
        if expanded.insert(current.to_owned()) {
            None
        } else {
            folder_after(at).filter(|&i| rows[i].depth > rows[at].depth)
        }
    } else if pressed(egui::Key::ArrowLeft) {
        if expanded.remove(current) {
            None
        } else {
            (0..at).rev().find(|&i| matches!(rows[i].kind, Kind::Folder { .. }) && rows[i].depth < rows[at].depth)
        }
    } else {
        if pressed(egui::Key::Enter) && !expanded.remove(current) {
            expanded.insert(current.to_owned());
        }
        None
    };
    target.map(|i| (rows[i].path.to_owned(), i))
}

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
    if let Some((path, row)) = keyboard(ui, &rows, current, expanded) {
        scroll_to = Some((row as f32 * ROW_H - ROW_H * 3.0).max(0.0));
        *action.borrow_mut() = Some(TreeAction::Navigate(path));
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

fn columns(rect: Rect) -> (f32, f32, f32) {
    let items_right = rect.right() - 8.0;
    let size_right = items_right - ITEMS_W;
    let bar_right = size_right - SIZE_W;
    (items_right, size_right, bar_right)
}

fn header(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, SURFACE);
    ui.painter().hline(rect.x_range(), rect.bottom(), Stroke::new(1.0_f32, LINE_STRONG));
    let (items_right, size_right, bar_right) = columns(rect);
    let label = |x: f32, align: Align2, text: &str| {
        ui.painter().text(egui::pos2(x, rect.center().y), align, tracked(text), sans(MICRO), MUTED);
    };
    label(rect.left() + 32.0, Align2::LEFT_CENTER, "Name");
    label(bar_right - BAR_W * 0.5, Align2::CENTER_CENTER, "Share");
    label(size_right, Align2::RIGHT_CENTER, "Size");
    label(items_right, Align2::RIGHT_CENTER, "Items");
}

fn paint_row(
    ui: &mut Ui,
    hierarchy: &Hierarchy,
    row: &Row,
    current: &str,
    expanded: &mut BTreeSet<String>,
    action: &RefCell<Option<TreeAction>>,
) {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::click());
    let folder_row = matches!(row.kind, Kind::Folder { .. });
    let selected = folder_row && row.path == current;
    if selected {
        ui.painter().rect_filled(rect, 0.0, SELECTED);
        ui.painter().rect_filled(Rect::from_min_size(rect.min, Vec2::new(3.0, rect.height())), 0.0, ACID_STRONG);
    } else if response.hovered() {
        ui.painter().rect_filled(rect, 0.0, HOVER);
    }
    let indent = rect.left() + 10.0 + row.depth as f32 * 14.0;
    let (items_right, size_right, bar_right) = columns(rect);
    let name_clip = Rect::from_min_max(rect.min, egui::pos2(bar_right - BAR_W - 8.0, rect.bottom()));
    let centre = rect.center().y;
    match &row.kind {
        Kind::Folder { size, count } => {
            let caret = Rect::from_center_size(egui::pos2(indent + 6.0, centre), Vec2::splat(20.0));
            let caret_hit = ui.interact(caret, Id::new(("tree-caret", row.path)), Sense::click());
            let open = expanded.contains(row.path);
            let expandable = hierarchy
                .folders
                .get(row.path)
                .map_or(*count > 0, |folder| !folder.children.is_empty() || !folder.direct_files.is_empty());
            if expandable {
                chevron(ui, caret.center(), open, caret_hit.hovered());
            }
            let name = if row.path == "/" { "Indexed space".to_owned() } else { path_name(row.path) };
            folder_glyph(ui, egui::pos2(indent + 22.0, centre));
            ui.painter().with_clip_rect(name_clip).text(
                egui::pos2(indent + 32.0, centre),
                Align2::LEFT_CENTER,
                name,
                sans(SMALL),
                TEXT,
            );
            stats(ui, rect, row.share, selected, *size, Some(*count), (items_right, size_right, bar_right));
            response.clone().on_hover_text(format!("{}\nClick to open, double-click or the arrow keys to expand", row.path));
            if caret_hit.clicked() {
                if !expanded.remove(row.path) {
                    expanded.insert(row.path.to_owned());
                }
            } else if response.double_clicked() {
                if !expanded.remove(row.path) {
                    expanded.insert(row.path.to_owned());
                }
            } else if response.clicked() {
                *action.borrow_mut() = Some(TreeAction::Navigate(row.path.to_owned()));
            }
        }
        Kind::File(file) => {
            let ext = path_name(&file.path).rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
            ui.painter().rect_stroke(
                Rect::from_center_size(egui::pos2(indent + 22.0, centre), Vec2::new(8.0, 10.0)),
                2.0,
                Stroke::new(1.0_f32, extension_color(&ext)),
                StrokeKind::Inside,
            );
            ui.painter().with_clip_rect(name_clip).text(
                egui::pos2(indent + 32.0, centre),
                Align2::LEFT_CENTER,
                path_name(&file.path),
                sans(SMALL),
                MUTED,
            );
            stats(ui, rect, row.share, false, file.size, None, (items_right, size_right, bar_right));
            if response.double_clicked() {
                *action.borrow_mut() = Some(TreeAction::Open(file.path.clone()));
            } else if response.clicked() {
                *action.borrow_mut() = Some(TreeAction::Select(file.path.clone()));
            }
            response.on_hover_text(format!("{}\nDouble-click to open", file.path));
        }
        Kind::More(hidden) => {
            let text = if *hidden > 0 { format!("{hidden} more files in this folder") } else { "Larger folders list their biggest files only".to_owned() };
            ui.painter().text(egui::pos2(indent + 22.0, centre), Align2::LEFT_CENTER, text, sans(CAPTION), MUTED);
        }
        Kind::Loading => {
            ui.painter().text(egui::pos2(indent + 22.0, centre), Align2::LEFT_CENTER, "Loading...", sans(CAPTION), MUTED);
        }
    }
}

fn stats(ui: &Ui, rect: Rect, fraction: f32, selected: bool, size: u64, count: Option<u64>, cols: (f32, f32, f32)) {
    let (items_right, size_right, bar_right) = cols;
    let centre = rect.center().y;
    let track = Rect::from_min_size(egui::pos2(bar_right - BAR_W, centre - 2.0), Vec2::new(BAR_W, 4.0));
    ui.painter().rect_filled(track, 2.0, LINE_STRONG);
    let filled = Rect::from_min_size(track.min, Vec2::new(BAR_W * fraction, 4.0));
    ui.painter().rect_filled(filled, 2.0, if selected { ACID } else { SUBTLE });
    ui.painter().text(egui::pos2(size_right, centre), Align2::RIGHT_CENTER, format_size(size), mono(CAPTION), MUTED);
    if let Some(count) = count {
        ui.painter().text(egui::pos2(items_right, centre), Align2::RIGHT_CENTER, short_count(count), mono(CAPTION), MUTED);
    }
}

fn chevron(ui: &Ui, centre: egui::Pos2, open: bool, hot: bool) {
    let points = if open {
        vec![centre + Vec2::new(-4.0, -2.0), centre + Vec2::new(4.0, -2.0), centre + Vec2::new(0.0, 3.0)]
    } else {
        vec![centre + Vec2::new(-2.0, -4.0), centre + Vec2::new(-2.0, 4.0), centre + Vec2::new(3.0, 0.0)]
    };
    ui.painter().add(egui::Shape::convex_polygon(points, if hot { TEXT } else { MUTED }, Stroke::NONE));
}

fn folder_glyph(ui: &Ui, centre: egui::Pos2) {
    let body = Rect::from_center_size(centre + Vec2::new(0.0, 1.0), Vec2::new(12.0, 8.0));
    ui.painter().rect_stroke(body, 2.0, Stroke::new(1.0_f32, WARN), StrokeKind::Inside);
    ui.painter().line_segment([body.left_top() + Vec2::new(0.0, -1.5), body.left_top() + Vec2::new(4.5, -1.5)], Stroke::new(1.5_f32, WARN));
}
