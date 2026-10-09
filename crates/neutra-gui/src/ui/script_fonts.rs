use super::*;

const KEY: &str = "neutra-script-fonts";

pub(super) fn remember(ctx: &egui::Context, definitions: &FontDefinitions) {
    ctx.data_mut(|data| data.insert_temp(Id::new(KEY), definitions.clone()));
}

pub(crate) fn ensure(ctx: &egui::Context, text: &str) {
    if text.is_ascii() {
        return;
    }
    let scripts = [
        ("neutra_arabic", text.chars().any(|ch| matches!(ch as u32, 0x0600..=0x08ff | 0xfb50..=0xfdff | 0xfe70..=0xfeff)),
            include_bytes!("../../assets/fonts/NotoSansArabic-Regular.ttf.zst").as_slice()),
        ("neutra_devanagari", text.chars().any(|ch| matches!(ch as u32, 0x0900..=0x097f)),
            include_bytes!("../../assets/fonts/NotoSansDevanagari-Regular.ttf.zst").as_slice()),
        ("neutra_cjk", text.chars().any(|ch| matches!(ch as u32, 0x2e80..=0x9fff | 0xac00..=0xd7ff | 0xf900..=0xfaff | 0x20000..=0x3134f)),
            include_bytes!("../../assets/fonts/NotoSansCJK-Regular.ttc.zst").as_slice()),
    ];
    let mut definitions = ctx
        .data(|data| data.get_temp::<FontDefinitions>(Id::new(KEY)))
        .unwrap_or_default();
    let mut changed = false;
    for (name, needed, bytes) in scripts {
        if !needed || definitions.font_data.contains_key(name) {
            continue;
        }
        let Ok(raw) = zstd::stream::decode_all(bytes) else {
            continue;
        };
        definitions
            .font_data
            .insert(name.into(), Arc::new(FontData::from_owned(raw)));
        for family in [
            FontFamily::Proportional,
            FontFamily::Monospace,
            FontFamily::Name("Neutra Sans".into()),
            FontFamily::Name("Neutra Mono".into()),
        ] {
            definitions
                .families
                .entry(family)
                .or_default()
                .push(name.into());
        }
        changed = true;
    }
    if changed {
        remember(ctx, &definitions);
        ctx.set_fonts(definitions);
        ctx.request_repaint();
    }
}
