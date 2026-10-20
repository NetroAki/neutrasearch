use super::super::*;

pub(crate) fn prepare(app: &mut NeutraApp, ui: &mut Ui) -> bool {
    if let Some(error) = app.lanes.get("tree").filter(|lane| lane.error) {
        let message = error.status.clone();
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("Cannot load the folder map").color(ERROR));
            ui.label(message);
            if ui.button("Retry folder map").clicked() {
                app.lanes.remove("tree");
                app.tree_pending.clear();
                ui.ctx().request_repaint();
            }
        });
        return false;
    }
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
        return false;
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
        return false;
    }
    true
}
