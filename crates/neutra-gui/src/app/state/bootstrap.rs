use super::*;

pub(super) fn restore_durable(cache_path: &std::path::Path) -> Restored {
    if !cache_path.is_file() {
        let index = std::fs::read(legacy_cache_path())
            .ok()
            .and_then(|bytes| Index::restore(&bytes).ok())
            .unwrap_or_default();
        return Restored {
            compact: None,
            index,
            cache_error: None,
        };
    }
    match CompactIndex::open_fast(cache_path) {
        Ok(compact) => Restored {
            compact: Some(std::sync::Arc::new(compact)),
            index: Index::default(),
            cache_error: None,
        },
        Err(error) => Restored {
            compact: None,
            index: Index::default(),
            cache_error: Some(format!(
                "cannot open durable index {}: {error}",
                cache_path.display()
            )),
        },
    }
}

pub(super) fn default_settings() -> GuiSettings {
    // Setup never asks which folders to scan: everything is indexed by
    // default and locations stay adjustable from the index settings.
    GuiSettings {
        onboarding_complete: true,
        roots: default_system_roots(),
        ..GuiSettings::default()
    }
}

pub(super) fn startup_query() -> String {
    std::env::var("NEUTRASEARCH_GUI_QUERY").unwrap_or_default()
}

pub(super) fn load_logo(ctx: &egui::Context) -> egui::TextureHandle {
    let (rgba, width, height) = embedded_logo();
    ctx.load_texture(
        "neutrasearch-logo",
        egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &rgba),
        egui::TextureOptions::LINEAR,
    )
}
