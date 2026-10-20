use super::*;

pub(super) fn show(app: &mut NeutraApp, ctx: &egui::Context) {
    let settings = &mut app.shortcuts;
    settings.poll();
    if !settings.open {
        return;
    }
    let mut open = true;
    egui::Window::new("Keyboard shortcuts")
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(440.0)
        .show(ctx, |ui| {
            ui.label(RichText::new("Quick search from any application").color(TEXT));
            ui.label(
                RichText::new("Default: Alt+K (Option+K on Mac). Other apps may already use it.")
                    .color(MUTED),
            );
            ui.add_space(8.0);
            ui.add_enabled_ui(settings.pending.is_none(), |ui| editor(settings, ui));
            if settings.pending.is_some() {
                ui.label("Applying shortcuts…");
            }
            if let Some(message) = &settings.message {
                match message {
                    Ok(()) if settings.keys.is_empty() => {
                        ui.label("Global quick search disabled");
                    }
                    Ok(()) => {
                        ui.label("Shortcuts saved and active");
                    }
                    Err(error) => {
                        ui.label(RichText::new(error).color(ERROR));
                    }
                }
            }
        });
    if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
        open = false;
    }
    settings.open = open;
}

fn editor(settings: &mut crate::shortcuts::ShortcutSettings, ui: &mut Ui) {
    let mut remove = None;
    let mut changed = false;
    for (index, key) in settings.keys.iter_mut().enumerate() {
        ui.push_id(index, |ui| {
            ui.horizontal(|ui| {
                let response = ui.add(egui::TextEdit::singleline(key).desired_width(220.0));
                changed |= response.changed();
                ui.ctx().accesskit_node_builder(response.id, |node| {
                    node.set_label(format!("Quick-search shortcut {}", index + 1))
                });
                if ui.button("Remove").clicked() {
                    remove = Some(index);
                }
            });
        });
    }
    if changed {
        settings.message = None;
    }
    if let Some(index) = remove {
        settings.keys.remove(index);
        settings.message = None;
    }
    ui.horizontal(|ui| {
        let response = ui.add(
            egui::TextEdit::singleline(&mut settings.input)
                .hint_text("e.g. Alt+J or Ctrl+Shift+Space")
                .desired_width(260.0),
        );
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_label("New quick-search shortcut")
        });
        if ui
            .add_enabled(settings.keys.len() < 8, egui::Button::new("Add shortcut"))
            .clicked()
        {
            let mut keys = settings.keys.clone();
            keys.push(settings.input.clone());
            match crate::shortcuts::normalize(&keys) {
                Ok(keys) => {
                    settings.keys = keys
                        .iter()
                        .map(|key| crate::shortcuts::keys::label(key))
                        .collect();
                    settings.input.clear();
                    settings.message = None;
                }
                Err(error) => settings.message = Some(Err(error)),
            }
        }
    });
    ui.label(RichText::new("Use Alt, Ctrl, Shift, Super (Windows key) or Command with a key. Remove all to disable quick search.").color(MUTED));
    ui.horizontal(|ui| {
        if ui.button("Restore default").clicked() {
            settings.keys = vec![crate::shortcuts::keys::label(crate::shortcuts::DEFAULT)];
            settings.input.clear();
            settings.message = None;
        }
        if ui.button("Apply shortcuts").clicked() {
            settings.apply(ui.ctx());
        }
    });
}
