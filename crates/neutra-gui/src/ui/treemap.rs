use super::*;
use std::collections::BTreeSet;
use super::hierarchy::{Hierarchy, TreeFile};


#[derive(Clone)]
struct MapBlock {
    path: String,
    name: String,
    bytes: u64,
    count: u64,
    folder: bool,
    extension: String,
}

pub(super) fn treemap_view(app: &mut NeutraApp, ui: &mut Ui) {
    if app.index_is_empty() {
        ui.centered_and_justified(|ui| {
            ui.vertical_centered(|ui| {
                if app.scanning {
                    ui.spinner();
                }
                let message = if app.scanning {
                    "Indexing system files..."
                } else {
                    "No indexed files yet"
                };
                ui.label(RichText::new(message).font(sans(11.0)).color(MUTED));
            });
        });
        return;
    }
     // Fetch-on-navigate: requests complete instantly when every visible
     // folder is cached, otherwise the missing branches render as they land.
     app.request_tree_model();
     if app.tree_model.is_none() {
        ui.centered_and_justified(|ui| {
            ui.vertical_centered(|ui| {
                ui.spinner();
                ui.label(
                    RichText::new("Preparing the indexed drive hierarchy...")
                        .font(sans(11.0))
                        .color(MUTED),
                );
            });
        });
        return;
    }
    let hierarchy = app.tree_model.take().expect("tree model checked above");
    if !hierarchy.folders.contains_key(&app.treemap_path) {
        app.treemap_path = "/".into();
    }
    treemap_legend(ui);
    let narrow = ui.available_width() < 820.0;
    let current_path = app.treemap_path.clone();
    let selected = app.selected.clone();
    let mut expanded = std::mem::take(&mut app.tree_expanded);
    for ancestor in ancestor_paths(&current_path) {
        expanded.insert(ancestor);
    }
    let navigation = std::cell::RefCell::<Option<TreeAction>>::new(None);
    if narrow {
        let mut fraction = app.tree_vertical_fraction;
        let split = ResizableSplit::new("treemap-vertical", &mut fraction, SplitAxis::Vertical)
            .show(
                ui,
                |ui| {
                    ui.with_layout(Layout::top_down(Align::LEFT), |ui| {
                        tree_panel(ui, &hierarchy, &current_path, &mut expanded, &navigation)
                    });
                },
                |ui| {
                    ui.with_layout(Layout::top_down(Align::LEFT), |ui| {
                        map_panel(
                            ui,
                            &hierarchy,
                            &current_path,
                            selected.as_deref(),
                            &navigation,
                        )
                    });
                },
            );
        if split.double_clicked() {
            fraction = 0.34;
        }
        split.on_hover_cursor(egui::CursorIcon::ResizeVertical);
        app.tree_vertical_fraction = fraction.clamp(0.18, 0.65);
    } else {
        let available_width = ui.available_width().max(1.0);
        let mut fraction = app.tree_fraction;
        let split = ResizableSplit::new("treemap-horizontal", &mut fraction, SplitAxis::Horizontal)
            .show(
                ui,
                |ui| {
                    ui.with_layout(Layout::top_down(Align::LEFT), |ui| {
                        tree_panel(ui, &hierarchy, &current_path, &mut expanded, &navigation)
                    });
                },
                |ui| {
                    ui.with_layout(Layout::top_down(Align::LEFT), |ui| {
                        map_panel(
                            ui,
                            &hierarchy,
                            &current_path,
                            selected.as_deref(),
                            &navigation,
                        )
                    });
                },
            );
        if split.double_clicked() {
            fraction = 268.0 / available_width;
        }
        split.on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
        let minimum = (220.0 / available_width).clamp(0.1, 0.8);
        let maximum = (360.0 / available_width).clamp(minimum, 0.9);
        app.tree_fraction = fraction.clamp(minimum, maximum);
    }
    app.tree_expanded = expanded;
    app.tree_model = Some(hierarchy);
    if let Some(action) = navigation.into_inner() {
        apply_tree_action(app, action);
    }
}

fn treemap_legend(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, CANVAS);
    let items = [
        ("PDF", extension_color("pdf")),
        ("Spreadsheet", extension_color("xlsx")),
        ("Document", extension_color("docx")),
        ("Archive", extension_color("zip")),
        ("Image", extension_color("png")),
        ("Audio", extension_color("mp3")),
        ("Folder", TEAL),
    ];
    let mut x = rect.left() + 5.0;
    for (label, color) in items {
        let swatch = Rect::from_min_size(egui::pos2(x, rect.center().y - 4.0), Vec2::splat(9.0));
        ui.painter().rect_filled(swatch, 0.0, color);
        ui.painter().rect_stroke(
            swatch,
            0.0,
            Stroke::new(1.0_f32, Color32::from_white_alpha(80)),
            StrokeKind::Inside,
        );
        ui.painter().text(
            swatch.right_center() + Vec2::new(5.0, 0.0),
            Align2::LEFT_CENTER,
            label,
            sans(9.0),
            MUTED,
        );
        x += 18.0 + label.len() as f32 * 5.8;
    }
    ui.painter().text(
        rect.right_center() - Vec2::new(8.0, 0.0),
        Align2::RIGHT_CENTER,
        "Area represents on-disk size",
        sans(9.0),
        MUTED,
    );
}

enum TreeAction {
    Navigate(String),
    Select(String),
    Open(String),
}

fn tree_panel(
    ui: &mut Ui,
    hierarchy: &Hierarchy,
    current_path: &str,
    expanded: &mut BTreeSet<String>,
    action: &std::cell::RefCell<Option<TreeAction>>,
) {
    ui.painter().rect_filled(ui.max_rect(), 0.0, SURFACE);
    let root = hierarchy.folders.get("/").cloned().unwrap_or_default();
    fixed_strip(ui, 31.0, SURFACE, |ui| {
        ui.add_space(8.0);
        ui.label(RichText::new("Indexed space").font(sans(11.0)).strong());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(7.0);
            ui.label(
                RichText::new(format_size(root.size))
                    .font(mono(9.0))
                    .color(MUTED),
            );
        });
    });
    // Flatten the expanded subtree and index the current folder files, so
    // only visible rows paint each frame.
    let mut folder_rows: Vec<(&str, usize)> = Vec::new();
    let mut stack = vec![("/", 0)];
    while let Some((path, depth)) = stack.pop() {
        folder_rows.push((path, depth));
        if expanded.contains(path) {
             if let Some(folder) = hierarchy.folders.get(path) {
                 for child in folder.children.iter().rev() {
                     stack.push((child.path.as_str(), depth + 1));
                 }
             }
        }
    }
    let file_depth = ancestor_paths(current_path).len();
    let files = hierarchy.folders.get(current_path).map_or(&[] as &[TreeFile], |folder| &folder.direct_files);
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show_rows(ui, 24.0, folder_rows.len() + files.len(), |ui, range| {
            for row in range {
                if row < folder_rows.len() {
                    let (path, depth) = folder_rows[row];
                    tree_row(ui, hierarchy, path, depth, current_path, expanded, action);
                } else if let Some(file) = files.get(row - folder_rows.len()) {
                    file_row(ui, file, file_depth, action);
                }
            }
        });
}

fn file_row(ui: &mut Ui, file: &TreeFile, depth: usize, action: &std::cell::RefCell<Option<TreeAction>>) {
    let (rect, response) = tree_line(ui, depth, &file.path, false, false);
    let clip = Rect::from_min_max(rect.min + Vec2::new(5.0, 0.0), egui::pos2((rect.right() - 68.0).max(rect.left()), rect.bottom()));
    ui.painter().with_clip_rect(clip).text(
        rect.left_center() + Vec2::new(22.0 + depth as f32 * 13.0, 0.0),
        Align2::LEFT_CENTER,
        path_name(&file.path),
        sans(9.5),
        MUTED,
    );
    ui.painter().text(rect.right_center() - Vec2::new(6.0, 0.0), Align2::RIGHT_CENTER, format_size(file.size), mono(8.0), MUTED);
    if response.double_clicked() {
        *action.borrow_mut() = Some(TreeAction::Open(file.path.clone()));
    } else if response.clicked() {
        *action.borrow_mut() = Some(TreeAction::Select(file.path.clone()));
    }
}

fn tree_row(
    ui: &mut Ui,
    hierarchy: &Hierarchy,
    path: &str,
    depth: usize,
    current: &str,
    expanded: &mut BTreeSet<String>,
    action: &std::cell::RefCell<Option<TreeAction>>,
) {
    let selected = path == current;
    let name = if path == "/" {
        "Indexed space".into()
    } else {
        path_name(path)
    };
    let has_children = hierarchy
        .folders
        .get(path)
        .is_some_and(|folder| !folder.children.is_empty() || !folder.direct_files.is_empty());
    let (rect, response) = tree_line(ui, depth, path, selected, has_children);
    let caret_rect = Rect::from_center_size(
        rect.left_center() + Vec2::new(10.0 + depth as f32 * 13.0, 0.0),
        Vec2::splat(18.0),
    );
    let caret_response = ui.interact(caret_rect, Id::new(("tree-caret", path)), Sense::click());
    if has_children {
        let points = if expanded.contains(path) {
            vec![
                caret_rect.center() - Vec2::new(3.0, 1.5),
                caret_rect.center() + Vec2::new(3.0, -1.5),
                caret_rect.center() + Vec2::new(0.0, 2.5),
            ]
        } else {
            vec![
                caret_rect.center() - Vec2::new(1.5, 3.0),
                caret_rect.center() + Vec2::new(-1.5, 3.0),
                caret_rect.center() + Vec2::new(2.5, 0.0),
            ]
        };
        ui.painter()
            .add(egui::Shape::convex_polygon(points, SUBTLE, Stroke::NONE));
    }
    ui.painter().text(
        caret_rect.right_center() + Vec2::new(3.0, 0.0),
        Align2::LEFT_CENTER,
        shorten(&name, 34),
        sans(9.5),
        if selected { TEXT } else { MUTED },
    );
    if let Some(folder) = hierarchy.folders.get(path) {
        ui.painter().text(
            rect.right_center() - Vec2::new(6.0, 0.0),
            Align2::RIGHT_CENTER,
            format_size(folder.size),
            mono(8.0),
            MUTED,
        );
    }
    if caret_response.clicked() {
        if !expanded.remove(path) {
            expanded.insert(path.to_owned());
        }
    } else if response.clicked() {
        expanded.insert(path.to_owned());
        *action.borrow_mut() = Some(TreeAction::Navigate(path.to_owned()));
    }
}

fn tree_line(
    ui: &mut Ui,
    depth: usize,
    id: &str,
    selected: bool,
    _folder: bool,
) -> (Rect, egui::Response) {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.0), Sense::click());
    let response =
        response.union(ui.interact(rect, Id::new(("tree-row", id, depth)), Sense::click()));
    if selected {
        ui.painter().rect_filled(rect, 0.0, ACTIVE);
    } else if response.hovered() {
        ui.painter().rect_filled(rect, 0.0, HOVER);
    }
    if selected {
        ui.painter().rect_stroke(
            rect,
            0.0,
            Stroke::new(1.0_f32, ACID_STRONG),
            StrokeKind::Inside,
        );
    }
    (rect, response)
}

fn map_panel(
    ui: &mut Ui,
    hierarchy: &Hierarchy,
    current_path: &str,
    selected_path: Option<&str>,
    action: &std::cell::RefCell<Option<TreeAction>>,
) {
    ui.painter().rect_filled(ui.max_rect(), 0.0, BLACK);
    breadcrumb(ui, hierarchy, current_path, action);
    let blocks = map_blocks(hierarchy, current_path);
    if blocks.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.label(
                RichText::new("This folder has no indexed children")
                    .font(sans(11.0))
                    .color(MUTED),
            )
        });
        return;
    }
    let (rect, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
    let mut layout = Vec::new();
    layout_map(&blocks, rect.shrink(3.0), &mut layout);
    for (block, tile) in layout {
        let tile = tile.shrink(1.0);
        let response = ui.interact(tile, Id::new(("map-tile", &block.path)), Sense::click());
        let base = if block.folder {
            TEAL
        } else {
            extension_color(&block.extension)
        };
        let selected = selected_path == Some(block.path.as_str());
        ui.painter().rect_filled(
            tile,
            0.0,
            if response.hovered() {
                base.gamma_multiply(1.14)
            } else {
                base
            },
        );
        ui.painter().rect_stroke(
            tile,
            0.0,
            Stroke::new(
                if selected { 2.0_f32 } else { 1.0_f32 },
                if selected {
                    ACID
                } else {
                    Color32::from_white_alpha(48)
                },
            ),
            StrokeKind::Inside,
        );
        if tile.width() > 62.0 && tile.height() > 34.0 {
            let prefix = if block.folder { "> " } else { "" };
            ui.painter().with_clip_rect(tile.shrink(5.0)).text(
                tile.left_top() + Vec2::new(5.0, 5.0),
                Align2::LEFT_TOP,
                format!("{prefix}{}", block.name),
                sans(9.5),
                Color32::WHITE,
            );
            ui.painter().text(
                tile.left_bottom() + Vec2::new(5.0, -5.0),
                Align2::LEFT_BOTTOM,
                format!("{} · {}", format_size(block.bytes), block.count),
                mono(8.0),
                Color32::from_white_alpha(205),
            );
        }
        response
            .clone()
            .on_hover_text(format!("{}\n{}", block.path, format_size(block.bytes)));
        if response.clicked() {
            *action.borrow_mut() = Some(if block.folder {
                TreeAction::Navigate(block.path.clone())
            } else {
                TreeAction::Select(block.path.clone())
            });
        }
        if response.double_clicked() && !block.folder {
            *action.borrow_mut() = Some(TreeAction::Open(block.path.clone()));
        }
    }
}

fn breadcrumb(
    ui: &mut Ui,
    hierarchy: &Hierarchy,
    current_path: &str,
    action: &std::cell::RefCell<Option<TreeAction>>,
) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 31.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, SURFACE);
    ui.painter().hline(
        rect.x_range(),
        rect.bottom(),
        Stroke::new(1.0_f32, LINE_STRONG),
    );
    let mut x = rect.left() + 7.0;
    let crumbs = ancestor_paths(current_path);
    for (index, path) in crumbs.iter().enumerate() {
        let label = if path == "/" {
            "Indexed space".to_owned()
        } else {
            path_name(path)
        };
        let width = 13.0 + label.chars().count() as f32 * 6.0;
        let button = Rect::from_min_size(egui::pos2(x, rect.top() + 3.0), Vec2::new(width, 24.0));
        let response = ui.interact(button, Id::new(("crumb", path)), Sense::click());
        if response.hovered() {
            ui.painter().rect_filled(button, 1.0, HOVER);
        }
        ui.painter().text(
            button.center(),
            Align2::CENTER_CENTER,
            &label,
            sans(9.5),
            MUTED,
        );
        if response.clicked() {
            *action.borrow_mut() = Some(TreeAction::Navigate(path.clone()));
        }
        x += width;
        if index + 1 < crumbs.len() {
            ui.painter().text(
                egui::pos2(x + 4.0, rect.center().y),
                Align2::CENTER_CENTER,
                ">",
                mono(9.0),
                MUTED,
            );
            x += 12.0;
        }
    }
     if let Some(folder) = hierarchy.folders.get(current_path) {
         let label = if folder.files_truncated {
             format!(
                 "{} indexed · top {} files",
                 format_size(folder.size),
                 folder.direct_files.len()
             )
         } else {
             format!("{} indexed", format_size(folder.size))
         };
         ui.painter().text(
             rect.right_center() - Vec2::new(8.0, 0.0),
             Align2::RIGHT_CENTER,
             label,
             mono(8.5),
             MUTED,
         );
     }
}

fn map_blocks(hierarchy: &Hierarchy, current: &str) -> Vec<MapBlock> {
    enum Child { Dir(usize), File(usize) }
     let Some(folder) = hierarchy.folders.get(current) else { return Vec::new(); };
     // Subdirectory totals ride along in the parent listing, so tiles never
     // force-fetch child directories.
     let mut candidates: Vec<(u64, Child)> = Vec::new();
     for (index, child) in folder.children.iter().enumerate() {
         candidates.push((child.size.max(1), Child::Dir(index)));
     }
     for (index, file) in folder.direct_files.iter().enumerate() {
         candidates.push((file.size.max(1), Child::File(index)));
     }
    if candidates.len() > 256 {
        candidates.select_nth_unstable_by_key(255, |(bytes, _)| std::cmp::Reverse(*bytes));
        candidates.truncate(256);
    }
    candidates.sort_unstable_by_key(|candidate| std::cmp::Reverse(candidate.0));
    candidates.into_iter().map(|(_, child)| match child {
         Child::Dir(index) => {
             let child = &folder.children[index];
             MapBlock { path: child.path.clone(), name: path_name(&child.path), bytes: child.size.max(1), count: child.count, folder: true, extension: String::new() }
         }
        Child::File(index) => {
            let file = &folder.direct_files[index];
            let name = path_name(&file.path);
            MapBlock { path: file.path.clone(), name: name.clone(), bytes: file.size.max(1), count: 1, folder: false, extension: name.rsplit_once('.').map_or(String::new(), |(_, extension)| extension.to_ascii_lowercase()) }
        }
    }).collect()
}

fn apply_tree_action(app: &mut NeutraApp, action: TreeAction) {
    match action {
        TreeAction::Navigate(path) => {
            for ancestor in ancestor_paths(&path) {
                app.tree_expanded.insert(ancestor);
            }
            app.treemap_path = path;
        }
        TreeAction::Select(path) => {
            app.selected = Some(path);
        }
        TreeAction::Open(path) => perform_file_action(app, FileAction::Open(PathBuf::from(path))),
    }
}

fn layout_map<'a>(items: &'a [MapBlock], rect: Rect, out: &mut Vec<(&'a MapBlock, Rect)>) {
    if items.is_empty() || rect.width() < 2.0 || rect.height() < 2.0 {
        return;
    }
    if items.len() == 1 {
        out.push((&items[0], rect));
        return;
    }
    let total = items.iter().map(|item| item.bytes).sum::<u64>().max(1);
    let mut left = 0u64;
    let mut split = 1usize;
    for (index, item) in items.iter().enumerate().take(items.len() - 1) {
        left = left.saturating_add(item.bytes);
        split = index + 1;
        if left >= total / 2 {
            break;
        }
    }
    let ratio = (left as f32 / total as f32).clamp(0.08, 0.92);
    let (first, second) = if rect.width() >= rect.height() {
        let x = rect.left() + rect.width() * ratio;
        (
            Rect::from_min_max(rect.min, egui::pos2(x, rect.bottom())),
            Rect::from_min_max(egui::pos2(x, rect.top()), rect.max),
        )
    } else {
        let y = rect.top() + rect.height() * ratio;
        (
            Rect::from_min_max(rect.min, egui::pos2(rect.right(), y)),
            Rect::from_min_max(egui::pos2(rect.left(), y), rect.max),
        )
    };
    layout_map(&items[..split], first, out);
    layout_map(&items[split..], second, out);
}
