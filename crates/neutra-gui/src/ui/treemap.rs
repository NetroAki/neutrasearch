use super::*;
use super::hierarchy::{Hierarchy};
use super::tree_panel::tree_panel;


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
                    RichText::new(if app.tree_summary_pending {
                        "Building the folder map (once per index update)..."
                    } else {
                        "Preparing the indexed drive hierarchy..."
                    })
                        .font(sans(11.0))
                        .color(MUTED),
                );
            });
        });
        return;
    }
    let hierarchy = app.tree_model.take().expect("tree model checked above");
    // A folder that is still loading must not bounce the view back to the root.
    if !hierarchy.folders.contains_key(&app.treemap_path) && !app.tree_building {
        app.treemap_path = "/".into();
    }
    treemap_legend(ui);
    let narrow = ui.available_width() < 820.0;
    let current_path = app.treemap_path.clone();
    let selected = app.selected.clone();
    let mut expanded = std::mem::take(&mut app.tree_expanded);
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
            fraction = 380.0 / available_width;
        }
        split.on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
        let minimum = (340.0 / available_width).clamp(0.1, 0.8);
        let maximum = (560.0 / available_width).clamp(minimum, 0.9);
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
        ("Document", extension_color("docx")),
        ("Archive", extension_color("zip")),
        ("Image", extension_color("png")),
        ("Audio", extension_color("mp3")),
        ("Folder", WARN),
    ];
    let mut x = rect.left() + 5.0;
    for (label, color) in items {
        let swatch = Rect::from_min_size(egui::pos2(x, rect.center().y - 4.0), Vec2::splat(9.0));
        ui.painter().rect_filled(swatch, 2.0, color.gamma_multiply(0.34));
        ui.painter().rect_stroke(
            swatch,
            2.0,
            Stroke::new(1.0_f32, color.gamma_multiply(0.8)),
            StrokeKind::Inside,
        );
        ui.painter().text(
            swatch.right_center() + Vec2::new(5.0, 0.0),
            Align2::LEFT_CENTER,
            label,
            sans(CAPTION),
            MUTED,
        );
        x += 26.0 + label.len() as f32 * 6.8;
    }
    ui.painter().text(
        rect.right_center() - Vec2::new(8.0, 0.0),
        Align2::RIGHT_CENTER,
        "Area represents on-disk size",
        sans(CAPTION),
        MUTED,
    );
}

pub(super) enum TreeAction {
    Navigate(String),
    Select(String),
    Open(String),
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
        let loaded = hierarchy.folders.contains_key(current_path);
        ui.centered_and_justified(|ui| {
            ui.label(
                RichText::new(if loaded { "This folder has no indexed children" } else { "Loading..." })
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
            WARN
        } else {
            extension_color(&block.extension)
        };
        let selected = selected_path == Some(block.path.as_str());
        ui.painter().rect_filled(
            tile,
            2.0,
            base.gamma_multiply(if response.hovered() { 0.5 } else { 0.34 }),
        );
        ui.painter().rect_stroke(
            tile,
            2.0,
            Stroke::new(
                if selected { 2.0_f32 } else { 1.0_f32 },
                if selected {
                    ACID
                } else {
                    base.gamma_multiply(0.8)
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
                sans(CAPTION),
                TEXT,
            );
            ui.painter().text(
                tile.left_bottom() + Vec2::new(5.0, -5.0),
                Align2::LEFT_BOTTOM,
                format!("{} · {}", format_size(block.bytes), block.count),
                mono(10.0),
                MUTED,
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
        let width = 13.0 + label.chars().count() as f32 * 6.6;
        let button = Rect::from_min_size(egui::pos2(x, rect.top() + 3.0), Vec2::new(width, 24.0));
        let response = ui.interact(button, Id::new(("crumb", path)), Sense::click());
        if response.hovered() {
            ui.painter().rect_filled(button, 1.0, HOVER);
        }
        ui.painter().text(
            button.center(),
            Align2::CENTER_CENTER,
            &label,
            sans(CAPTION),
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
                mono(CAPTION),
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
             mono(CAPTION),
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
            // Opening a folder reveals it in the tree but leaves its own
            // expansion alone; that is the chevron's job.
            for ancestor in ancestor_paths(&path).into_iter().filter(|a| *a != path) {
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
