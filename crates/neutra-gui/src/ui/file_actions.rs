//! Shared explorer file menu plus the rename/Open-With dialogs.
//!
use crate::transport::file_actions::Action;
use egui::{Context, Ui};
use std::path::PathBuf;

pub(crate) fn context_menu(ui: &mut Ui, path: &str, selected: &mut Option<Action>) {
    let path_buf = PathBuf::from(path);
    for (label, action) in [
        ("Open", Action::Open(path_buf.clone())),
        (
            "Open with\u{2026}",
            Action::OpenWith {
                path: path_buf.clone(),
                desktop_id: String::new(),
            },
        ),
        ("Reveal in file manager", Action::Reveal(path_buf.clone())),
        (
            "Rename\u{2026}",
            Action::Rename {
                from: path_buf.clone(),
                to: path_buf.clone(),
            },
        ),
        ("Copy files", Action::Copy(vec![path_buf.clone()])),
        ("Cut files", Action::Cut(vec![path_buf.clone()])),
        ("Paste here", Action::Paste(path_buf.clone())),
        ("Move to Trash", Action::Trash(path_buf)),
    ] {
        let allowed = path_buf_has_parent(path)
            || !matches!(
                action,
                Action::Rename { .. } | Action::Trash(_) | Action::Cut(_)
            );
        if ui.add_enabled(allowed, egui::Button::new(label)).clicked() {
            *selected = Some(action);
            ui.close();
        }
    }
    if ui.button("Copy full path    Ctrl+Insert").clicked() {
        ui.ctx().copy_text(path.to_owned());
        ui.close();
    }
}

fn path_buf_has_parent(path: &str) -> bool {
    let path = std::path::Path::new(path);
    path.parent()
        .is_some_and(|parent| parent != path && !parent.as_os_str().is_empty())
}

pub(crate) fn dialogs(ops: &mut crate::app::file_operations::FileOperations, ctx: &Context) {
    if let Some(path) = ops.rename.clone() {
        egui::Window::new("Rename file")
            .default_width(420.0)
            .max_width(540.0)
            .collapsible(false)
            .resizable(true)
            .min_width(260.0)
            .show(ctx, |ui| {
                ui.add_sized(
                    [ui.available_width(), 36.0],
                    egui::Label::new(path.display().to_string()).truncate(),
                );
                let response = ui
                    .push_id("rename-name", |ui| {
                        ui.text_edit_singleline(&mut ops.rename_name)
                    })
                    .inner;
                if ops.rename_focus_pending {
                    response.request_focus();
                    ops.rename_focus_pending = false;
                }
                if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
                    ops.rename = None;
                }
                if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                    ops.confirm_rename();
                }
                ui.horizontal(|ui| {
                    if ui.button("Rename").clicked() {
                        ops.confirm_rename();
                    }
                    if ui.button("Cancel").clicked() {
                        ops.rename = None;
                    }
                });
            });
    }
    if let Some((path, apps)) = ops.open_with.clone() {
        egui::Window::new("Open with")
            .default_width(420.0)
            .max_width(540.0)
            .collapsible(false)
            .resizable(true)
            .min_width(260.0)
            .show(ctx, |ui| {
                ui.add_sized(
                    [ui.available_width(), 36.0],
                    egui::Label::new(path.display().to_string()).truncate(),
                );
                if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
                    ops.open_with = None;
                }
                if apps.is_empty() {
                    ui.label("No associated desktop applications were found.");
                }
                for app in apps {
                    if ui.button(&app.name).clicked() {
                        ops.choose_open_with(app.id);
                    }
                }
                if ui.button("Cancel").clicked() {
                    ops.open_with = None;
                }
            });
    }
}
