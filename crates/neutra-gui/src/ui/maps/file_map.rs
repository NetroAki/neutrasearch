use super::super::*;
use std::collections::BTreeMap;

pub(super) fn show(
    ui: &mut Ui,
    files: &[FileRecord],
    selected: Option<&str>,
) -> Option<TreeAction> {
    if files.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.label(RichText::new("No indexed files in this location").color(MUTED))
        });
        return None;
    }
    let mut grouped: BTreeMap<String, Vec<MapBlock>> = BTreeMap::new();
    for file in files {
        let extension = file.extension().to_ascii_lowercase();
        grouped
            .entry(extension.clone())
            .or_default()
            .push(MapBlock {
                path: file.path.to_string(),
                name: file.name().to_owned(),
                bytes: file.disk_bytes(),
                count: 1,
                folder: false,
                extension,
            });
    }
    let mut groups: Vec<MapBlock> = grouped
        .iter()
        .map(|(extension, files)| MapBlock {
            path: extension.clone(),
            name: if extension.is_empty() {
                "No extension".into()
            } else {
                format!(".{extension}")
            },
            bytes: files.iter().map(|file| file.bytes).sum(),
            count: files.len() as u64,
            folder: false,
            extension: extension.clone(),
        })
        .collect();
    groups.sort_unstable_by_key(|group| std::cmp::Reverse(group.bytes));
    ui.label(
        RichText::new(format!(
            "{} indexed files shown · grouped by format · area represents occupied space",
            files.len()
        ))
        .font(sans(CAPTION))
        .color(MUTED),
    );
    if files.len() == 2048 {
        ui.label(
            RichText::new(
                "Showing 2,048 files in this location. Choose a folder to inspect a smaller area.",
            )
            .font(sans(CAPTION))
            .color(MUTED),
        );
    }
    let (rect, _) =
        ui.allocate_exact_size(ui.available_size().max(Vec2::splat(1.0)), Sense::hover());
    let mut groups_layout = Vec::new();
    layout_map(&groups, rect.shrink(2.0), &mut groups_layout);
    let mut action = None;
    for (group, rect) in groups_layout {
        let rect = rect.shrink(2.0);
        if rect.width() < 3.0 || rect.height() < 3.0 {
            continue;
        }
        let color = extension_color(&group.extension);
        ui.painter().rect_filled(rect, 2.0, SURFACE);
        let header = if rect.height() > 48.0 && rect.width() > 70.0 {
            18.0
        } else {
            0.0
        };
        if header > 0.0 {
            ui.painter().with_clip_rect(rect).text(
                rect.left_top() + Vec2::new(4.0, 2.0),
                Align2::LEFT_TOP,
                format!("{} · {}", group.name, format_size(group.bytes)),
                sans(MICRO),
                TEXT,
            );
        }
        let inner = Rect::from_min_max(rect.min + Vec2::new(0.0, header), rect.max);
        let mut tiles = Vec::new();
        layout_map(&grouped[&group.extension], inner, &mut tiles);
        for (file, tile) in tiles {
            let tile = tile.shrink(0.5);
            if tile.width() <= 1.0 || tile.height() <= 1.0 {
                continue;
            }
            let response = ui.interact(
                tile,
                Id::new(("format-file", &file.path)),
                Sense::click_and_drag(),
            );
            crate::ui::file_interactions::interact(ui, &response, &file.path, false);
            ui.painter().rect_filled(
                tile,
                1.0,
                color.gamma_multiply(if response.hovered() { 0.5 } else { 0.3 }),
            );
            if selected == Some(file.path.as_str()) || response.has_focus() {
                ui.painter()
                    .rect_stroke(tile, 1.0, Stroke::new(2.0_f32, GLOW), StrokeKind::Inside);
            }
            if tile.width() > 75.0 && tile.height() > 24.0 {
                ui.painter().with_clip_rect(tile.shrink(3.0)).text(
                    tile.center(),
                    Align2::CENTER_CENTER,
                    &file.name,
                    sans(CAPTION),
                    TEXT,
                );
            }
            response
                .clone()
                .on_hover_text(format!("{}\n{}", file.path, format_size(file.bytes)));
            if response.clicked() {
                action = Some(TreeAction::Select(file.path.clone()));
            }
            if response.double_clicked() {
                action = Some(TreeAction::Open(file.path.clone()));
            }
            response.context_menu(|menu| file_context_menu(menu, &file.path, &mut action));
        }
    }
    action
}
