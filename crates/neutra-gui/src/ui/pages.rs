use super::*;

pub(super) fn controls(app: &mut NeutraApp, ui: &mut Ui) {
    if app.query.trim().is_empty() && !app.index_is_empty() {
        let first = app.page_offset + usize::from(!app.hits.is_empty());
        ui.label(
            RichText::new(format!(
                "Items {}–{}",
                fmt_count(first as u64),
                fmt_count((app.page_offset + app.hits.len()) as u64)
            ))
            .font(sans(CAPTION))
            .color(MUTED),
        );
        if ui
            .add_enabled(
                !app.searching && app.page_offset > 0,
                egui::Button::new("Previous").frame(false),
            )
            .clicked()
        {
            app.page_offset = app.page_offset.saturating_sub(crate::HOME_RESULT_CAP);
            app.page_cursors.pop();
            app.requery();
        }
        if ui
            .add_enabled(
                !app.searching && app.hits.len() == crate::HOME_RESULT_CAP,
                egui::Button::new("Next").frame(false),
            )
            .clicked()
        {
            app.page_cursors
                .push(app.hits.last().map(|hit| hit.record.clone()));
            app.page_offset += crate::HOME_RESULT_CAP;
            app.requery();
        }
    }
}
