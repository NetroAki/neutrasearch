#[path = "maps/ball_graph.rs"]
mod ball_graph;
#[path = "maps/file_map.rs"]
mod file_map;
#[path = "maps/navigation.rs"]
mod navigation;
#[path = "maps/tiles.rs"]
mod tiles;
pub(super) use navigation::breadcrumb;
pub(super) use tiles::{layout_map, map_blocks, MapBlock};

use super::*;

/// 0 is the existing folder view, 1 groups visible indexed files by format,
/// and 2 shows the cached hierarchy as a bounded graph.
pub(super) fn view_picker(ui: &mut Ui) -> usize {
    ui.horizontal(|ui| {
        ui.label(RichText::new("MAP STYLE").font(sans(MICRO)).color(MUTED));
        let mut mode = ui
            .ctx()
            .data(|data| data.get_temp::<usize>(Id::new("map-style")).unwrap_or(0));
        for (index, label) in ["Folders", "File formats", "Ball graph"]
            .into_iter()
            .enumerate()
        {
            if ui.selectable_label(mode == index, label).clicked() {
                mode = index;
            }
        }
        ui.ctx()
            .data_mut(|data| data.insert_temp(Id::new("map-style"), mode));
        mode
    })
    .inner
}

pub(super) fn format_view(
    ui: &mut Ui,
    files: &[FileRecord],
    selected: Option<&str>,
) -> Option<super::TreeAction> {
    file_map::show(ui, files, selected)
}

pub(super) fn ball_view(
    ui: &mut Ui,
    model: &Hierarchy,
    path: &str,
    selected: Option<&str>,
) -> Option<super::TreeAction> {
    ball_graph::show(ui, model, path, selected)
}
