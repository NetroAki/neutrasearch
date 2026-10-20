//! Shared chrome-free building blocks: buttons, painted icons, and
//! path/count/size formatting. Tokens and the egui theme live in `theme.rs`
//! and are re-exported here for the view modules.

pub(crate) use super::theme::*;
use super::*;

pub(crate) fn sans(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("Neutra Sans".into()))
}

pub(crate) fn mono(size: f32) -> FontId {
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

pub(crate) fn paint_search_icon(ui: &mut Ui, color: Color32) {
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
        egui::Button::new(RichText::new(label).font(sans(SMALL)).color(if active {
            ACID
        } else {
            MUTED
        }))
        .fill(if active { SELECTED } else { SURFACE })
        .stroke(Stroke::new(
            1.0_f32,
            if active {
                LINE_STRONG
            } else {
                Color32::TRANSPARENT
            },
        ))
        .corner_radius(4)
        .min_size(Vec2::new(0.0, 25.0)),
    )
}

pub(super) fn primary_button(ui: &mut Ui, label: &str, color: Color32) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).font(sans(SMALL)).color(BLACK).strong())
            .fill(color)
            .stroke(Stroke::new(1.0_f32, color))
            .corner_radius(4)
            .min_size(Vec2::new(0.0, 32.0)),
    )
}

pub(super) fn secondary_button(ui: &mut Ui, label: &str, color: Color32) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).font(sans(SMALL)).color(TEXT))
            .fill(RAISED)
            .stroke(Stroke::new(
                1.0_f32,
                if color == MUTED { LINE_STRONG } else { color },
            ))
            .corner_radius(4)
            .min_size(Vec2::new(0.0, 32.0)),
    )
}

mod paths;
pub(super) use paths::*;

pub(crate) fn type_badge(record: &neutra_core::FileRecord) -> String {
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

pub(crate) fn type_color(record: &neutra_core::FileRecord) -> Color32 {
    if record.kind == FileKind::Dir {
        return WARN;
    }
    extension_color(record.extension())
}

pub(super) fn extension_color(extension: &str) -> Color32 {
    match extension.to_ascii_lowercase().as_str() {
        "pdf" => ERROR,
        "xls" | "xlsx" | "ods" | "csv" | "doc" | "docx" | "txt" | "md" | "rtf" => MUTED,
        "zip" | "7z" | "rar" | "tar" | "gz" | "pak" | "iso" => WARN,
        "mp3" | "wav" | "flac" | "ogg" | "aif" | "aiff" | "m4a" | "opus" => VIOLET,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "mp4" | "mkv" => GREEN,
        _ => MUTED,
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
    if age < 0 {
        let (year, month, day) = civil_date(timestamp.div_euclid(86_400));
        format!("{year:04}-{month:02}-{day:02}")
    } else if age < 60 {
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
