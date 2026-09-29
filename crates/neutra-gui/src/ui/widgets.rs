//! Shared chrome-free building blocks: theme configuration, fonts, buttons,
//! painted icons, and path/count/size formatting. Split from the view modules
//! (`ui.rs`, `ui/results.rs`, `ui/treemap.rs`) which compose them.

use super::*;

pub(super) const BLACK: Color32 = Color32::from_rgb(10, 13, 19);
pub(crate) const CANVAS: Color32 = Color32::from_rgb(22, 27, 37);
pub(super) const SURFACE: Color32 = Color32::from_rgb(21, 27, 38);
pub(super) const RAISED: Color32 = Color32::from_rgb(28, 35, 48);
pub(super) const HOVER: Color32 = Color32::from_rgb(36, 44, 60);
pub(super) const ACTIVE: Color32 = Color32::from_rgb(44, 54, 74);
pub(super) const TEXT: Color32 = Color32::from_rgb(230, 233, 239);
pub(super) const MUTED: Color32 = Color32::from_rgb(139, 147, 163);
pub(super) const SUBTLE: Color32 = Color32::from_rgb(103, 111, 128);
pub(super) const LINE: Color32 = Color32::from_rgb(32, 39, 53);
pub(super) const LINE_STRONG: Color32 = Color32::from_rgb(52, 62, 82);
pub(super) const ACID: Color32 = Color32::from_rgb(96, 150, 250);
pub(super) const ACID_STRONG: Color32 = Color32::from_rgb(47, 124, 246);
pub(super) const BLUE: Color32 = Color32::from_rgb(96, 150, 250);
pub(super) const BLUE_DIM: Color32 = Color32::from_rgb(24, 38, 62);
pub(super) const WARN: Color32 = Color32::from_rgb(224, 178, 74);
pub(super) const WARN_DIM: Color32 = Color32::from_rgb(67, 53, 28);
pub(super) const ERROR: Color32 = Color32::from_rgb(229, 83, 75);
pub(super) const ERROR_DIM: Color32 = Color32::from_rgb(66, 28, 30);


pub(crate) fn configure(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    register_font(
        &mut fonts,
        "neutra_sans",
        load_font(include_bytes!("../../assets/fonts/NotoSans-Regular.ttf.zst")),
    );
    register_font(
        &mut fonts,
        "neutra_mono",
        load_font(include_bytes!("../../assets/fonts/NotoSansMono-Regular.ttf.zst")),
    );
    register_font(
        &mut fonts,
        "neutra_arabic",
        load_font(include_bytes!("../../assets/fonts/NotoSansArabic-Regular.ttf.zst")),
    );
    register_font(
        &mut fonts,
        "neutra_devanagari",
        load_font(include_bytes!("../../assets/fonts/NotoSansDevanagari-Regular.ttf.zst")),
    );
    register_font(
        &mut fonts,
        "neutra_cjk",
        load_font(include_bytes!("../../assets/fonts/NotoSansCJK-Regular.ttc.zst")),
    );
    register_font(
        &mut fonts,
        "neutra_symbols",
        load_font(include_bytes!("../../assets/fonts/NotoSansSymbols-Regular.ttf.zst")),
    );
    register_font(
        &mut fonts,
        "neutra_symbols2",
        load_font(include_bytes!("../../assets/fonts/NotoSansSymbols2-Regular.ttf.zst")),
    );

    let proportional = vec![
        "neutra_sans",
        "neutra_arabic",
        "neutra_devanagari",
        "neutra_cjk",
        "neutra_symbols",
        "neutra_symbols2",
    ];
    let monospace = vec![
        "neutra_mono",
        "neutra_sans",
        "neutra_arabic",
        "neutra_devanagari",
        "neutra_cjk",
        "neutra_symbols",
        "neutra_symbols2",
    ];
    fonts.families.insert(
        FontFamily::Name("Neutra Sans".into()),
        proportional.iter().map(|name| (*name).to_owned()).collect(),
    );
    fonts.families.insert(
        FontFamily::Name("Neutra Mono".into()),
        monospace.iter().map(|name| (*name).to_owned()).collect(),
    );
    for name in proportional.into_iter().rev() {
        fonts
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, name.to_owned());
    }
    for name in monospace.into_iter().rev() {
        fonts
            .families
            .entry(FontFamily::Monospace)
            .or_default()
            .insert(0, name.to_owned());
    }
    ctx.set_fonts(fonts);

    Theme::dark().store(ctx);
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = CANVAS;
    visuals.window_fill = SURFACE;
    visuals.extreme_bg_color = BLACK;
    visuals.faint_bg_color = SURFACE;
    visuals.selection.bg_fill = ACTIVE;
    visuals.selection.stroke = Stroke::new(1.0_f32, ACID);
    visuals.widgets.noninteractive.bg_fill = SURFACE;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, LINE);
    visuals.widgets.noninteractive.corner_radius = 2.into();
    visuals.widgets.inactive.bg_fill = RAISED;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, LINE_STRONG);
    visuals.widgets.inactive.corner_radius = 2.into();
    visuals.widgets.hovered.bg_fill = HOVER;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, LINE_STRONG);
    visuals.widgets.hovered.corner_radius = 2.into();
    visuals.widgets.active.bg_fill = ACTIVE;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, ACID_STRONG);
    visuals.widgets.active.corner_radius = 2.into();
    visuals.widgets.open.bg_fill = HOVER;
    visuals.widgets.open.bg_stroke = Stroke::new(1.0_f32, ACID_STRONG);
    visuals.override_text_color = Some(TEXT);
    visuals.window_corner_radius = 3.into();
    visuals.menu_corner_radius = 2.into();
    visuals.popup_shadow = egui::epaint::Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha(150),
    };
    ctx.set_visuals(visuals);

    let sans_family = FontFamily::Name("Neutra Sans".into());
    let mono_family = FontFamily::Name("Neutra Mono".into());
    let mut style = (*ctx.global_style()).clone();
    style.spacing.item_spacing = Vec2::new(4.0, 3.0);
    style.spacing.button_padding = Vec2::new(8.0, 4.0);
    style.spacing.interact_size = Vec2::new(30.0, 30.0);
    style.spacing.menu_margin = Margin::same(5);
    style
        .text_styles
        .insert(TextStyle::Small, FontId::new(10.0, sans_family.clone()));
    style
        .text_styles
        .insert(TextStyle::Body, FontId::new(12.0, sans_family.clone()));
    style
        .text_styles
        .insert(TextStyle::Button, FontId::new(11.0, sans_family.clone()));
    style
        .text_styles
        .insert(TextStyle::Heading, FontId::new(18.0, sans_family));
    style
        .text_styles
        .insert(TextStyle::Monospace, FontId::new(11.0, mono_family));
    style.visuals = ctx.global_style().visuals.clone();
    ctx.set_global_style(style);
}

fn load_font(compressed: &'static [u8]) -> Vec<u8> {
    zstd::stream::decode_all(compressed).expect("embedded font must decompress")
}

fn register_font(fonts: &mut FontDefinitions, name: &str, bytes: Vec<u8>) {
    fonts
        .font_data
        .insert(name.to_owned(), Arc::new(FontData::from_owned(bytes)));
}

pub(super) fn sans(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("Neutra Sans".into()))
}

pub(super) fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("Neutra Mono".into()))
}

pub(super) fn fixed_strip(ui: &mut Ui, height: f32, fill: Color32, add: impl FnOnce(&mut Ui)) {
    let width = ui.available_width();
    ui.allocate_ui_with_layout(
        Vec2::new(width, height),
        Layout::left_to_right(Align::Center),
        |ui| {
            let rect = ui.max_rect();
            ui.painter().rect_filled(rect, 0.0, fill);
            ui.painter()
                .hline(rect.x_range(), rect.bottom(), Stroke::new(1.0_f32, LINE));
            add(ui);
        },
    );
}

pub(super) fn copy_to_clipboard(ui: &Ui, text: &str) {
    ui.ctx().copy_text(text.to_owned());
    if let Ok(mut clipboard) = arboard::Clipboard::new() {
        let _ = clipboard.set_text(text.to_owned());
    }
}

pub(super) fn paint_search_icon(ui: &mut Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(19.0), Sense::hover());
    let center = rect.center() - Vec2::new(1.5, 1.5);
    ui.painter()
        .circle_stroke(center, 5.0, Stroke::new(1.5_f32, color));
    ui.painter().line_segment(
        [center + Vec2::new(3.7, 3.7), center + Vec2::new(7.0, 7.0)],
        Stroke::new(1.5_f32, color),
    );
}

pub(super) fn task_icon(ui: &mut Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(40.0), Sense::hover());
    ui.painter().rect_filled(
        rect,
        3.0,
        Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 28),
    );
    ui.painter()
        .rect_stroke(rect, 3.0, Stroke::new(1.0_f32, color), StrokeKind::Inside);
    let inner = rect.shrink(5.0);
    let folder = Rect::from_min_size(inner.min + Vec2::new(3.0, 8.0), Vec2::new(24.0, 17.0));
    ui.painter()
        .rect_stroke(folder, 1.0, Stroke::new(1.5_f32, color), StrokeKind::Inside);
    ui.painter().line_segment(
        [folder.left_top(), folder.left_top() + Vec2::new(9.0, -4.0)],
        Stroke::new(1.5_f32, color),
    );
}

pub(super) fn segment_button(ui: &mut Ui, label: &str, active: bool) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).font(sans(10.0)).color(if active {
            ACID
        } else {
            MUTED
        }))
        .fill(if active { ACTIVE } else { SURFACE })
        .stroke(Stroke::new(
            1.0_f32,
            if active {
                LINE_STRONG
            } else {
                Color32::TRANSPARENT
            },
        ))
        .corner_radius(1)
        .min_size(Vec2::new(0.0, 25.0)),
    )
}

pub(super) fn primary_button(ui: &mut Ui, label: &str, color: Color32) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).font(sans(10.5)).color(BLACK).strong())
            .fill(color)
            .stroke(Stroke::new(1.0_f32, color))
            .corner_radius(2)
            .min_size(Vec2::new(0.0, 32.0)),
    )
}

pub(super) fn secondary_button(ui: &mut Ui, label: &str, color: Color32) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).font(sans(10.5)).color(TEXT))
            .fill(RAISED)
            .stroke(Stroke::new(
                1.0_f32,
                if color == MUTED { LINE_STRONG } else { color },
            ))
            .corner_radius(2)
            .min_size(Vec2::new(0.0, 32.0)),
    )
}

pub(super) fn is_drive_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'/'
}

pub(super) fn normalize_path(path: &str) -> String {
    let replaced = path.replace('\\', "/");
    let unc = replaced.starts_with("//");
    let mut normalized = replaced
        .split('/')
        .filter(|component| !component.is_empty())
        .collect::<Vec<_>>()
        .join("/");
    if unc {
        normalized.insert_str(0, "//");
    } else if replaced.starts_with('/') {
        normalized.insert(0, '/');
    }
    if is_drive_path(&replaced) && normalized.len() == 2 {
        normalized.push('/');
    }
    if normalized.is_empty() {
        "/".into()
    } else {
        normalized
    }
}

pub(super) fn parent_path(path: &str) -> String {
    let normalized = normalize_path(path);
    if normalized == "/" || is_volume_root(&normalized) {
        return "/".into();
    }
    normalized.rsplit_once('/').map_or_else(
        || "/".into(),
        |(parent, _)| {
            if parent.is_empty() {
                "/".into()
            } else if parent.len() == 2 && parent.ends_with(':') {
                format!("{parent}/")
            } else {
                parent.into()
            }
        },
    )
}

pub(super) fn is_volume_root(path: &str) -> bool {
    if is_drive_path(path) {
        return path.len() == 3;
    }
    if let Some(unc) = path.strip_prefix("//") {
        return unc.split('/').filter(|part| !part.is_empty()).count() == 2;
    }
    false
}

pub(super) fn path_name(path: &str) -> String {
    let normalized = normalize_path(path);
    if normalized == "/" {
        return "Computer".into();
    }
    if is_volume_root(&normalized) {
        return normalized.trim_end_matches('/').to_owned();
    }
    normalized
        .rsplit('/')
        .next()
        .unwrap_or(&normalized)
        .to_owned()
}

pub(super) fn ancestor_paths(path: &str) -> Vec<String> {
    let normalized = normalize_path(path);
    let mut out = vec!["/".to_owned()];
    if is_drive_path(&normalized) {
        let root = normalized[..3].to_owned();
        out.push(root.clone());
        let mut current = root;
        for component in normalized[3..]
            .split('/')
            .filter(|component| !component.is_empty())
        {
            current.push_str(component);
            out.push(current.clone());
            current.push('/');
        }
    } else if let Some(unc) = normalized.strip_prefix("//") {
        let mut components = unc.split('/').filter(|component| !component.is_empty());
        if let (Some(server), Some(share)) = (components.next(), components.next()) {
            let mut current = format!("//{server}/{share}");
            out.push(current.clone());
            for component in components {
                current.push('/');
                current.push_str(component);
                out.push(current.clone());
            }
        }
    } else {
        let mut current = String::new();
        for component in normalized
            .split('/')
            .filter(|component| !component.is_empty())
        {
            current.push('/');
            current.push_str(component);
            out.push(current.clone());
        }
    }
    out.dedup();
    out
}

pub(super) fn type_badge(record: &neutra_core::FileRecord) -> String {
    if record.kind == FileKind::Dir {
        return "DIR".into();
    }
    let ext = record.extension();
    if ext.is_empty() {
        "FILE".into()
    } else {
        ext.chars().take(4).collect::<String>().to_ascii_uppercase()
    }
}

pub(super) fn type_color(record: &neutra_core::FileRecord) -> Color32 {
    if record.kind == FileKind::Dir {
        return BLUE;
    }
    extension_color(record.extension())
}

pub(super) fn extension_color(extension: &str) -> Color32 {
    match extension.to_ascii_lowercase().as_str() {
        "pdf" => ERROR,
        "xls" | "xlsx" | "ods" | "csv" => ACID,
        "doc" | "docx" | "txt" | "md" | "rtf" => BLUE,
        "zip" | "7z" | "rar" | "tar" | "gz" | "pak" | "iso" => WARN,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "mp4" | "mkv" => {
            Color32::from_rgb(190, 112, 203)
        }
        _ => Color32::from_rgb(128, 146, 153),
    }
}

pub(super) fn format_mtime(timestamp: i64) -> String {
    if timestamp <= 0 {
        return "Unknown".into();
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(timestamp, |duration| duration.as_secs() as i64);
    let age = now.saturating_sub(timestamp);
    if age < 60 {
        "Just now".into()
    } else if age < 3_600 {
        format!("{} min ago", age / 60)
    } else if age < 86_400 {
        format!("{} hr ago", age / 3_600)
    } else if age < 604_800 {
        format!("{} days ago", age / 86_400)
    } else {
        let (year, month, day) = civil_date(timestamp.div_euclid(86_400));
        format!("{year:04}-{month:02}-{day:02}")
    }
}

pub(super) fn civil_date(days_since_epoch: i64) -> (i64, i64, i64) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

pub(super) fn fmt_count(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

pub(super) fn format_size(bytes: u64) -> String {
    if bytes >= 1 << 40 {
        format!("{:.1} TB", bytes as f64 / (1u64 << 40) as f64)
    } else if bytes >= 1 << 30 {
        format!("{:.1} GB", bytes as f64 / (1u64 << 30) as f64)
    } else if bytes >= 1 << 20 {
        format!("{:.1} MB", bytes as f64 / (1u64 << 20) as f64)
    } else if bytes >= 1 << 10 {
        format!("{:.1} KB", bytes as f64 / (1u64 << 10) as f64)
    } else {
        format!("{bytes} B")
    }
}

pub(super) fn shorten(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    let tail = value
        .chars()
        .rev()
        .take(max_chars.saturating_sub(1))
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("…{tail}")
}
