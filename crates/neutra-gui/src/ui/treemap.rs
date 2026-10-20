use super::hierarchy::Hierarchy;
use super::tree_panel::tree_panel;
use super::*;
#[path = "maps.rs"]
mod maps;
use maps::{layout_map, map_blocks, MapBlock};

pub(super) fn treemap_view(app: &mut NeutraApp, ui: &mut Ui) {
    if app.lanes.get("search").is_some_and(|lane| lane.error)
        || (app.regex_mode && !app.query.is_empty() && app.matcher.is_none())
    {
        super::results::empty_results(app, ui);
        return;
    }
    if !maps::prepare(app, ui) {
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
    Select(String, egui::Modifiers),
    Open(String),
    FileOperation(crate::transport::file_actions::Action),
    Focus(String, Id),
}

pub(super) fn file_context_menu(menu: &mut Ui, path: &str, action: &mut Option<TreeAction>) {
    let mut selected: Option<crate::transport::file_actions::Action> = None;
    let paths = menu
        .ctx()
        .data(|data| data.get_temp::<Arc<Vec<PathBuf>>>(Id::new("selected-file-paths")));
    if let Some(paths) = paths.filter(|paths| {
        paths
            .iter()
            .any(|selected| selected == std::path::Path::new(path))
    }) {
        crate::ui::file_actions::selection_menu(menu, path, &paths, &mut selected);
    } else {
        crate::ui::file_actions::context_menu(menu, path, &mut selected);
    }
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
    maps::folder_view(ui, hierarchy, current_path, selected_path, action);
}

fn apply_tree_action(app: &mut NeutraApp, action: TreeAction) {
    match action {
        TreeAction::Navigate(path) => {
            app.select_single(&path);
            // Opening a folder reveals it in the tree but leaves its own
            // expansion alone; that is the chevron's job.
            for ancestor in ancestor_paths(&path).into_iter().filter(|a| *a != path) {
                app.tree_expanded.insert(ancestor);
            }
            app.treemap_path = path;
        }
        TreeAction::Select(path, modifiers) => {
            let order: Vec<_> = if !app.map_files.is_empty() {
                app.map_files
                    .iter()
                    .map(|file| file.path.as_ref())
                    .collect()
            } else {
                app.tree_model
                    .as_ref()
                    .and_then(|model| model.folders.get(&app.treemap_path))
                    .map_or_else(Vec::new, |folder| {
                        folder
                            .direct_files
                            .iter()
                            .map(|file| file.path.as_str())
                            .collect()
                    })
            };
            app.selection
                .click(&path, modifiers.command, modifiers.shift, &order);
            app.selected = Some(path);
        }
        TreeAction::Focus(path, id) => {
            app.file_focus = Some(id);
            app.select_single(&path);
        }
        TreeAction::Open(path) => perform_file_action(app, FileAction::Open(PathBuf::from(path))),
        TreeAction::FileOperation(action) => app.file_operations.dispatch(action),
    }
}

#[cfg(test)]
mod tests;
