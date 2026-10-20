use super::super::*;

pub(crate) fn show(
    ui: &mut Ui,
    hierarchy: &Hierarchy,
    current_path: &str,
    selected_path: Option<&str>,
    action: &std::cell::RefCell<Option<TreeAction>>,
) {
    let selected_paths = ui.ctx().data(|data| {
        data.get_temp::<Arc<Vec<PathBuf>>>(Id::new("selected-file-paths"))
            .unwrap_or_default()
    });
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
            crate::ui::file_interactions::interact(ui, &response, &block.path, block.folder)
        {
            *action.borrow_mut() = Some(TreeAction::FileOperation(next));
        }
        let base = if block.folder {
            WARN
        } else {
            extension_color(&block.extension)
        };
        let selected = selected_path == Some(block.path.as_str())
            || selected_paths
                .iter()
                .any(|path| path == std::path::Path::new(&block.path));
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::SelectableLabel,
                true,
                selected,
                format!("{}; {}", block.path, format_size(block.bytes)),
            )
        });
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
                TreeAction::Select(block.path.clone(), ui.input(|input| input.modifiers))
            });
        }
        if response.double_clicked() && !block.folder {
            *action.borrow_mut() = Some(TreeAction::Open(block.path.clone()));
        }
        crate::ui::file_actions::popup(&response, |menu| {
            file_context_menu(menu, &block.path, &mut action.borrow_mut())
        });
    }
}
