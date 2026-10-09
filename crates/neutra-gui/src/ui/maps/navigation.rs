use super::super::Hierarchy;
use super::super::*;

pub(in crate::ui::treemap) fn breadcrumb(
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
    let summary = hierarchy.folders.get(current_path).map(|folder| {
        if folder.files_truncated {
            format!(
                "{} indexed · top {} files",
                format_size(folder.size),
                folder.direct_files.len()
            )
        } else {
            format!("{} indexed", format_size(folder.size))
        }
    });
    let summary_width = summary
        .as_ref()
        .map_or(0.0, |label| {
            ui.painter()
                .layout_no_wrap(label.clone(), mono(CAPTION), MUTED)
                .size()
                .x
                + 16.0
        })
        .min(rect.width() * 0.45);
    let trail = Rect::from_min_max(
        rect.min + Vec2::new(7.0, 3.0),
        egui::pos2(rect.right() - summary_width, rect.bottom() - 3.0),
    );
    ui.scope_builder(egui::UiBuilder::new().max_rect(trail), |ui| {
        ui.set_clip_rect(trail);
        egui::ScrollArea::horizontal()
            .id_salt("folder-breadcrumbs")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (index, path) in ancestor_paths(current_path).iter().enumerate() {
                        if index > 0 {
                            ui.label(RichText::new(">").font(mono(CAPTION)).color(MUTED));
                        }
                        let label = if path == "/" {
                            "Indexed space".to_owned()
                        } else {
                            path_name(path)
                        };
                        let response = ui
                            .add(
                                egui::Button::new(
                                    RichText::new(label).font(sans(CAPTION)).color(MUTED),
                                )
                                .frame(false),
                            )
                            .on_hover_text(path);
                        if response.clicked() {
                            *action.borrow_mut() = Some(TreeAction::Navigate(path.clone()));
                        }
                    }
                });
            });
    });
    if let Some(label) = summary {
        let painter = ui.painter().with_clip_rect(Rect::from_min_max(
            egui::pos2(rect.right() - summary_width, rect.top()),
            rect.max,
        ));
        painter.text(
            rect.right_center() - Vec2::new(8.0, 0.0),
            Align2::RIGHT_CENTER,
            label,
            mono(CAPTION),
            MUTED,
        );
    }
}
