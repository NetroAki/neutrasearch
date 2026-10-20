use crate::transport::file_actions::Action;
use egui::{Response, Ui};
use std::path::{Path, PathBuf};

pub(super) fn interact(ui: &Ui, response: &Response, path: &str, folder: bool) -> Option<Action> {
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect,
            3.0,
            egui::Stroke::new(2.0_f32, super::GLOW),
            egui::StrokeKind::Inside,
        );
    }
    if response.drag_started() {
        let selected = ui.ctx().data(|data| {
            data.get_temp::<std::sync::Arc<Vec<PathBuf>>>(egui::Id::new("selected-file-paths"))
        });
        let paths = selected
            .filter(|paths| paths.iter().any(|selected| selected == Path::new(path)))
            .map_or_else(|| vec![PathBuf::from(path)], |paths| (*paths).clone());
        response.dnd_set_drag_payload(paths.clone());
        ui.ctx()
            .data_mut(|data| data.insert_temp(egui::Id::new("native-file-drag-request"), paths));
    }
    if !folder {
        return None;
    }
    if response.dnd_hover_payload::<Vec<PathBuf>>().is_some() {
        ui.painter().rect_stroke(
            response.rect,
            3.0,
            egui::Stroke::new(2.0_f32, super::GLOW),
            egui::StrokeKind::Inside,
        );
    }
    let paths = response
        .dnd_release_payload::<Vec<PathBuf>>()
        .map(|paths| (*paths).clone())
        .or_else(|| {
            ui.rect_contains_pointer(response.rect).then(|| {
                ui.input(|input| {
                    input
                        .raw
                        .dropped_files
                        .iter()
                        .filter_map(|file| file.path.clone())
                        .collect::<Vec<_>>()
                })
            })
        })?;
    if paths.is_empty() || paths.iter().any(|source| source == Path::new(path)) {
        return None;
    }
    Some(Action::Transfer {
        paths,
        destination: PathBuf::from(path),
        cut: false,
    })
}
