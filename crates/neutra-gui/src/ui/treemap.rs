use super::hierarchy::Hierarchy;
use super::tree_panel::tree_panel;
use super::*;
#[path = "maps.rs"]
mod maps;
use maps::{layout_map, map_blocks, MapBlock};

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
    let mode = maps::view_picker(ui);
    if mode == 1 {
        let smallest = ui.ctx().data(|data| {
            data.get_temp::<bool>(Id::new("map-smallest"))
                .unwrap_or(false)
        });
        crate::app::map_queries::request(app, smallest);
    }
    // A folder that is still loading must not bounce the view back to the root.
    if !hierarchy.folders.contains_key(&app.treemap_path) && !app.tree_building {
        app.treemap_path = "/".into();
    }
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
                            mode,
                            (&app.map_files, app.map_files_pending),
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
                            mode,
                            (&app.map_files, app.map_files_pending),
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

pub(super) enum TreeAction {
    Navigate(String),
    Select(String),
    Open(String),
    FileOperation(crate::transport::file_actions::Action),
}

pub(super) fn file_context_menu(menu: &mut Ui, path: &str, action: &mut Option<TreeAction>) {
    let mut selected: Option<crate::transport::file_actions::Action> = None;
    crate::ui::file_actions::context_menu(menu, path, &mut selected);
    if let Some(selected) = selected {
        use crate::transport::file_actions::Action;
        *action = Some(match selected {
            Action::Open(path) => TreeAction::Open(path.to_string_lossy().into_owned()),
            other => TreeAction::FileOperation(other),
        });
    }
}

fn map_panel(
    ui: &mut Ui,
    hierarchy: &Hierarchy,
    current_path: &str,
    selected_path: Option<&str>,
    mode: usize,
    files: (&[FileRecord], bool),
    action: &std::cell::RefCell<Option<TreeAction>>,
) {
    let (files, files_pending) = files;
    ui.painter().rect_filled(ui.max_rect(), 0.0, BLACK);
    maps::breadcrumb(ui, hierarchy, current_path, action);
    if mode == 1 {
        ui.horizontal(|ui| {
            let mut smallest = ui.ctx().data(|data| {
                data.get_temp::<bool>(Id::new("map-smallest"))
                    .unwrap_or(false)
            });
            ui.selectable_value(&mut smallest, false, "Largest files");
            ui.selectable_value(&mut smallest, true, "Smallest files");
            ui.ctx()
                .data_mut(|data| data.insert_temp(Id::new("map-smallest"), smallest));
        });
        if files_pending {
            ui.label(RichText::new("Loading indexed file sizes…").color(MUTED));
        }
        if let Some(next) = maps::format_view(ui, files, selected_path) {
            *action.borrow_mut() = Some(next);
        }
        return;
    }
    if mode == 2 {
        if let Some(next) = maps::ball_view(ui, hierarchy, current_path, selected_path) {
            *action.borrow_mut() = Some(next);
        }
        return;
    }
    let blocks = map_blocks(hierarchy, current_path);
    if blocks.is_empty() {
        let loaded = hierarchy.folders.contains_key(current_path);
        ui.centered_and_justified(|ui| {
            ui.label(
                RichText::new(if loaded {
                    "This folder has no indexed children"
                } else {
                    "Loading..."
                })
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
        let response = ui.interact(
            tile,
            Id::new(("map-tile", &block.path)),
            Sense::click_and_drag(),
        );
        if let Some(next) =
            super::file_interactions::interact(ui, &response, &block.path, block.folder)
        {
            *action.borrow_mut() = Some(TreeAction::FileOperation(next));
        }
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
        response
            .context_menu(|menu| file_context_menu(menu, &block.path, &mut action.borrow_mut()));
    }
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
        TreeAction::FileOperation(action) => app.file_operations.dispatch(action),
    }
}

#[cfg(test)]
mod tests {
    use super::super::hierarchy::{FolderSummary, Hierarchy};
    use super::*;

    #[test]
    fn empty_and_deep_ancestor_paths_are_safe() {
        let model = Hierarchy::empty();
        assert!(maps::map_blocks(&model, "/").is_empty());
        assert_eq!(
            ancestor_paths("/a/b"),
            vec!["/".to_owned(), "/a".to_owned(), "/a/b".to_owned()]
        );
        let mut model = model;
        model.folders.insert("/".into(), FolderSummary::default());
        assert!(maps::map_blocks(&model, "/").is_empty());
    }
}
