use super::*;

pub(super) fn picker(app: &mut NeutraApp, ui: &mut Ui) {
    let label = if let Some(path) = &app.scope_root {
        format!("Search in: {}", shorten(path, 26))
    } else if app.search_roots.is_empty() {
        "Search in: All drives".into()
    } else {
        format!("Search in: {} locations", app.search_roots.len())
    };
    ui.menu_button(label, |ui| {
        if ui
            .selectable_label(
                app.search_roots.is_empty() && app.scope_root.is_none(),
                "All indexed drives",
            )
            .clicked()
        {
            app.search_roots.clear();
            app.search_excluded.clear();
            app.scope_root = None;
            app.requery();
            ui.close();
        }
        let mounts = neutra_core::mounts::system_mounts().unwrap_or_default();
        let nvme: Vec<_> = mounts
            .iter()
            .filter(|mount| mount.device.contains("nvme") && mount.fs.is_indexable_local())
            .map(|mount| mount.mountpoint.clone())
            .collect();
        if !nvme.is_empty() && ui.button("NVMe drives only").clicked() {
            app.search_excluded = mounts
                .iter()
                .filter(|mount| mount.fs.is_indexable_local() && !nvme.contains(&mount.mountpoint))
                .map(|mount| mount.mountpoint.clone())
                .collect();
            app.search_roots = nvme;
            app.scope_root = None;
            app.requery();
            ui.close();
        }
        ui.separator();
        let mut roots: Vec<PathBuf> = mounts
            .iter()
            .filter(|mount| {
                mount.fs.is_indexable_local()
                    && crate::scope_within_selected_roots(
                        &mount.mountpoint.to_string_lossy(),
                        &app.selected_roots,
                    )
            })
            .map(|mount| mount.mountpoint.clone())
            .collect();
        roots.extend(app.selected_roots.iter().cloned());
        roots.sort();
        roots.dedup();
        for root in roots {
            let mut selected = app.search_roots.contains(&root);
            if ui
                .checkbox(&mut selected, root.display().to_string())
                .changed()
            {
                app.scope_root = None;
                app.search_excluded.clear();
                if selected {
                    app.search_roots.push(root);
                } else {
                    app.search_roots.retain(|candidate| candidate != &root);
                }
                app.requery();
            }
        }
        ui.separator();
        ui.label(
            RichText::new("Select one or more indexed locations.")
                .font(sans(CAPTION))
                .color(MUTED),
        );
        if ui.button("Search a folder…").clicked() {
            if let Some(folder) = rfd::FileDialog::new()
                .set_title("Search within folder")
                .pick_folder()
            {
                app.scope_root = Some(folder.to_string_lossy().into_owned());
                app.search_roots.clear();
                app.search_excluded.clear();
                app.requery();
                ui.close();
            }
        }
    });
}
