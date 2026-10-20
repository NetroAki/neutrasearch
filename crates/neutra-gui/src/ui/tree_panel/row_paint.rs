use super::*;

pub(super) fn paint_row(
    ui: &mut Ui,
    hierarchy: &Hierarchy,
    row: &Row,
    current: &str,
    expanded: &mut BTreeSet<String>,
    action: &RefCell<Option<TreeAction>>,
) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::hover());
    let folder_row = matches!(row.kind, Kind::Folder { .. });
    let selected = row.path == current
        || ui
            .ctx()
            .data(|data| data.get_temp::<Arc<Vec<PathBuf>>>(Id::new("selected-file-paths")))
            .is_some_and(|paths| {
                paths
                    .iter()
                    .any(|path| path == std::path::Path::new(row.path))
            });
    let interactive = matches!(row.kind, Kind::Folder { .. } | Kind::File(_));
    let response = ui.interact(
        rect,
        Id::new(("tree-row", row.path)),
        if interactive {
            Sense::click_and_drag()
        } else {
            Sense::hover()
        },
    );
    if interactive {
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::SelectableLabel,
                true,
                row.path == current,
                row.path,
            )
        });
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_role(egui::accesskit::Role::TreeItem);
            node.set_level(row.depth + 1);
            node.set_selected(selected);
            if folder_row {
                node.set_expanded(expanded.contains(row.path));
            }
        });
        if response.gained_focus() {
            *action.borrow_mut() = Some(TreeAction::Focus(row.path.into(), response.id));
        }
    }
    if let Some(next) = super::file_interactions::interact(ui, &response, row.path, folder_row) {
        *action.borrow_mut() = Some(TreeAction::FileOperation(next));
    }
    if selected {
        ui.painter().rect_filled(rect, 0.0, SELECTED);
        ui.painter().rect_filled(
            Rect::from_min_size(rect.min, Vec2::new(3.0, rect.height())),
            0.0,
            ACID_STRONG,
        );
    } else if response.hovered() {
        ui.painter().rect_filled(rect, 0.0, HOVER);
    }
    let layout = row_layout(rect, row.depth);
    let indent = layout.indent;
    let name_clip = Rect::from_min_max(rect.min, egui::pos2(layout.name_right, rect.bottom()));
    let centre = rect.center().y;
    match &row.kind {
        Kind::Folder { size, count } => {
            let caret = Rect::from_center_size(egui::pos2(indent + 6.0, centre), Vec2::splat(20.0));
            let open = expanded.contains(row.path);
            let expandable = hierarchy
                .folders
                .get(row.path)
                .map_or(*count > 0, |folder| {
                    !folder.children.is_empty() || !folder.direct_files.is_empty()
                });
            let caret_hit = expandable
                .then(|| ui.interact(caret, Id::new(("tree-caret", row.path)), Sense::click()));
            if let Some(hit) = &caret_hit {
                hit.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::WidgetType::Button,
                        true,
                        format!(
                            "{} {}",
                            if open { "Collapse" } else { "Expand" },
                            path_name(row.path)
                        ),
                    )
                });
            }
            if expandable {
                chevron(
                    ui,
                    caret.center(),
                    open,
                    caret_hit.as_ref().is_some_and(|hit| hit.hovered()),
                );
            }
            let name = if row.path == "/" {
                "Indexed space".to_owned()
            } else {
                path_name(row.path)
            };
            folder_glyph(ui, egui::pos2(indent + 22.0, centre));
            ui.painter().with_clip_rect(name_clip).text(
                egui::pos2(indent + 32.0, centre),
                Align2::LEFT_CENTER,
                name,
                sans(SMALL),
                TEXT,
            );
            stats(ui, rect, row.share, selected, *size, Some(*count), layout);
            let full_path = format!(
                "{}\nClick to open, double-click or the arrow keys to expand",
                row.path
            );
            response.clone().on_hover_text(full_path.clone());
            if response.has_focus() {
                response.clone().show_tooltip_text(full_path);
            }
            if caret_hit.as_ref().is_some_and(|hit| hit.clicked()) || response.double_clicked() {
                if !expanded.remove(row.path) {
                    expanded.insert(row.path.to_owned());
                }
            } else if response.clicked() {
                *action.borrow_mut() = Some(TreeAction::Navigate(row.path.to_owned()));
            }
            super::file_actions::popup(&response, |menu| {
                super::treemap::file_context_menu(menu, row.path, &mut action.borrow_mut())
            });
        }
        Kind::File(file) => {
            let ext = path_name(&file.path)
                .rsplit_once('.')
                .map(|(_, e)| e.to_ascii_lowercase())
                .unwrap_or_default();
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
            stats(ui, rect, row.share, false, file.size, None, layout);
            if response.double_clicked() {
                *action.borrow_mut() = Some(TreeAction::Open(file.path.clone()));
            } else if response.clicked() {
                *action.borrow_mut() = Some(TreeAction::Select(
                    file.path.clone(),
                    ui.input(|input| input.modifiers),
                ));
            }
            let full_path = format!("{}\nDouble-click to open", file.path);
            response.clone().on_hover_text(full_path.clone());
            if response.has_focus() {
                response.clone().show_tooltip_text(full_path);
            }
            super::file_actions::popup(&response, |menu| {
                super::treemap::file_context_menu(menu, &file.path, &mut action.borrow_mut())
            });
        }
        Kind::More(hidden) => {
            let text = if *hidden > 0 {
                format!("{hidden} more files in this folder")
            } else {
                "Larger folders list their biggest files only".to_owned()
            };
            ui.painter().text(
                egui::pos2(indent + 22.0, centre),
                Align2::LEFT_CENTER,
                text,
                sans(CAPTION),
                MUTED,
            );
        }
        Kind::Loading => {
            ui.painter().text(
                egui::pos2(indent + 22.0, centre),
                Align2::LEFT_CENTER,
                "Loading...",
                sans(CAPTION),
                MUTED,
            );
        }
    }
}

pub(super) fn stats(
    ui: &Ui,
    rect: Rect,
    fraction: f32,
    selected: bool,
    size: u64,
    count: Option<u64>,
    cols: RowLayout,
) {
    let centre = rect.center().y;
    if let Some(bar_right) = cols.bar_right {
        let track = Rect::from_min_size(
            egui::pos2(bar_right - cols.bar_width, centre - 2.0),
            Vec2::new(cols.bar_width, 4.0),
        );
        ui.painter().rect_filled(track, 2.0, LINE_STRONG);
        let filled = Rect::from_min_size(track.min, Vec2::new(cols.bar_width * fraction, 4.0));
        ui.painter()
            .rect_filled(filled, 2.0, if selected { ACID } else { SUBTLE });
    }
    if let Some(size_right) = cols.size_right {
        ui.painter().text(
            egui::pos2(size_right, centre),
            Align2::RIGHT_CENTER,
            format_size(size),
            mono(CAPTION),
            MUTED,
        );
    }
    if let (Some(items_right), Some(count)) = (cols.items_right, count) {
        ui.painter().text(
            egui::pos2(items_right, centre),
            Align2::RIGHT_CENTER,
            short_count(count),
            mono(CAPTION),
            MUTED,
        );
    }
}
