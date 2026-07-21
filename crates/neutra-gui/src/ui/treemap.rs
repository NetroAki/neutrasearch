use super::*;

#[derive(Clone)]
struct TreeFile {
    path: String,
    name: String,
    size: u64,
    extension: String,
}

#[derive(Clone, Default)]
struct FolderSummary {
    size: u64,
    count: u64,
    children: BTreeSet<String>,
    direct_files: Vec<TreeFile>,
}

pub(crate) struct Hierarchy {
    folders: BTreeMap<String, FolderSummary>,
}

impl Hierarchy {
    pub(crate) fn from_records(records: &[FileRecord]) -> Self {
        let mut folders = BTreeMap::<String, FolderSummary>::new();
        folders.entry("/".into()).or_default();
        for record in records {
            let normalized = normalize_path(&record.path);
            let parent = parent_path(&normalized);
            let mut ancestors = ancestor_paths(&parent);
            if ancestors.is_empty() {
                ancestors.push("/".into());
            }
            if record.kind != FileKind::Dir {
                for ancestor in &ancestors {
                    let summary = folders.entry(ancestor.clone()).or_default();
                    summary.size = summary.size.saturating_add(record.size.max(1));
                    summary.count += 1;
                }
            }
            for pair in ancestors.windows(2) {
                folders
                    .entry(pair[0].clone())
                    .or_default()
                    .children
                    .insert(pair[1].clone());
            }
            if record.kind == FileKind::Dir {
                folders.entry(normalized.clone()).or_default();
                folders
                    .entry(parent)
                    .or_default()
                    .children
                    .insert(normalized);
            } else {
                let name = path_name(&normalized);
                let extension = name
                    .rsplit_once('.')
                    .map_or("", |(_, extension)| extension)
                    .to_ascii_lowercase();
                folders
                    .entry(parent)
                    .or_default()
                    .direct_files
                    .push(TreeFile {
                        path: record.path.to_string(),
                        name,
                        size: record.size,
                        extension,
                    });
            }
        }
        for folder in folders.values_mut() {
            folder
                .direct_files
                .sort_unstable_by_key(|file| std::cmp::Reverse(file.size));
        }
        Self { folders }
    }
}

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
    if app.tree_model.is_none() {
        app.request_tree_model();
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
        ("PDF", Color32::from_rgb(132, 52, 52)),
        ("Spreadsheet", Color32::from_rgb(54, 126, 87)),
        ("Document", Color32::from_rgb(57, 92, 145)),
        ("Archive", Color32::from_rgb(139, 105, 50)),
        ("Image", Color32::from_rgb(116, 64, 133)),
        ("Folder", Color32::from_rgb(62, 91, 101)),
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
        "Area represents indexed size",
        sans(9.0),
        SUBTLE,
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
        ui.label(RichText::new("Local disk").font(sans(11.0)).strong());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(7.0);
            ui.label(
                RichText::new(format_size(root.size))
                    .font(mono(9.0))
                    .color(MUTED),
            );
        });
    });
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            tree_row(ui, hierarchy, "/", 0, current_path, expanded, action);
            render_folder_children(ui, hierarchy, "/", 1, current_path, expanded, action);
            if let Some(folder) = hierarchy.folders.get(current_path) {
                let depth = ancestor_paths(current_path).len();
                for file in &folder.direct_files {
                    let (rect, response) = tree_line(ui, depth, &file.path, false, false);
                    let name_clip = Rect::from_min_max(
                        rect.min + Vec2::new(5.0, 0.0),
                        egui::pos2((rect.right() - 68.0).max(rect.left()), rect.bottom()),
                    );
                    ui.painter().with_clip_rect(name_clip).text(
                        rect.left_center() + Vec2::new(22.0 + depth as f32 * 13.0, 0.0),
                        Align2::LEFT_CENTER,
                        &file.name,
                        sans(9.5),
                        MUTED,
                    );
                    ui.painter().text(
                        rect.right_center() - Vec2::new(6.0, 0.0),
                        Align2::RIGHT_CENTER,
                        format_size(file.size),
                        mono(8.0),
                        SUBTLE,
                    );
                    if response.double_clicked() {
                        *action.borrow_mut() = Some(TreeAction::Open(file.path.clone()));
                    } else if response.clicked() {
                        *action.borrow_mut() = Some(TreeAction::Select(file.path.clone()));
                    }
                }
            }
        });
}

fn render_folder_children(
    ui: &mut Ui,
    hierarchy: &Hierarchy,
    parent: &str,
    depth: usize,
    current: &str,
    expanded: &mut BTreeSet<String>,
    action: &std::cell::RefCell<Option<TreeAction>>,
) {
    let Some(folder) = hierarchy.folders.get(parent) else {
        return;
    };
    for child in &folder.children {
        tree_row(ui, hierarchy, child, depth, current, expanded, action);
        if expanded.contains(child) {
            render_folder_children(ui, hierarchy, child, depth + 1, current, expanded, action);
        }
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
        "Local disk /".into()
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
            SUBTLE,
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
            Color32::from_rgb(62, 91, 101)
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
            "Local disk".to_owned()
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
                SUBTLE,
            );
            x += 12.0;
        }
    }
    if let Some(folder) = hierarchy.folders.get(current_path) {
        ui.painter().text(
            rect.right_center() - Vec2::new(8.0, 0.0),
            Align2::RIGHT_CENTER,
            format!("{} indexed", format_size(folder.size)),
            mono(8.5),
            SUBTLE,
        );
    }
}

fn map_blocks(hierarchy: &Hierarchy, current: &str) -> Vec<MapBlock> {
    let Some(folder) = hierarchy.folders.get(current) else {
        return Vec::new();
    };
    let mut blocks = Vec::new();
    for child in &folder.children {
        if let Some(summary) = hierarchy.folders.get(child) {
            blocks.push(MapBlock {
                path: child.clone(),
                name: path_name(child),
                bytes: summary.size.max(1),
                count: summary.count,
                folder: true,
                extension: String::new(),
            });
        }
    }
    for file in &folder.direct_files {
        blocks.push(MapBlock {
            path: file.path.clone(),
            name: file.name.clone(),
            bytes: file.size.max(1),
            count: 1,
            folder: false,
            extension: file.extension.clone(),
        });
    }
    blocks.sort_unstable_by_key(|block| std::cmp::Reverse(block.bytes));
    blocks.truncate(256);
    blocks
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
            if let Some(raw) = path
                .strip_prefix("file:")
                .and_then(|value| value.parse::<usize>().ok())
            {
                if let Some(hit) = app.hits.get(raw) {
                    app.selected = Some(hit.record.path.to_string());
                }
            } else {
                app.selected = Some(path);
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str) -> FileRecord {
        FileRecord {
            path: path.into(),
            size: 42,
            mtime: 0,
            mode: 0,
            kind: FileKind::File,
            fs: neutra_core::FsKind::Ntfs,
            native_id: 1,
            native_parent: 0,
            source: 0,
        }
    }

    #[test]
    fn hierarchy_connects_windows_drives_and_unc_shares_to_computer_root() {
        let hierarchy = Hierarchy::from_records(&[
            file(r"C:\\Users\\Alex\\report.txt"),
            file(r"\\\\server\\share\\team\\plan.txt"),
        ]);
        let root = hierarchy.folders.get("/").unwrap();
        assert!(root.children.contains("C:/"));
        assert!(root.children.contains("//server/share"));
        assert_eq!(
            hierarchy.folders["C:/Users/Alex"].direct_files[0].name,
            "report.txt"
        );
        assert_eq!(
            hierarchy.folders["//server/share/team"].direct_files[0].name,
            "plan.txt"
        );
    }
}
