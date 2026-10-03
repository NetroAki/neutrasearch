//! Design tokens and the egui theme. Values come from DESIGN.md (Neutraudio
//! spec §36 and the shell tokens in neutraudio-ui); change them there first.

use super::*;

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// surface-950: window background, recessed wells, text-edit fill.
pub(crate) const BLACK: Color32 = hex(0x020617);
/// surface-950: results canvas.
pub(crate) const CANVAS: Color32 = hex(0x020617);
/// surface-900: panels, toolbars, table header, status bar.
pub(crate) const SURFACE: Color32 = hex(0x0F172A);
/// surface-800: raised controls, selected row.
pub(crate) const RAISED: Color32 = hex(0x1E293B);
/// surface-800 at 50% over surface-900: row and control hover.
pub(crate) const HOVER: Color32 = hex(0x162032);
/// surface-700: pressed and open states.
pub(crate) const ACTIVE: Color32 = hex(0x334155);
/// surface-200.
pub(crate) const TEXT: Color32 = hex(0xE2E8F0);
/// surface-400: metadata and labels (6.96:1 on surface-900).
pub(crate) const MUTED: Color32 = hex(0x94A3B8);
/// surface-500: icons and decoration only. It is 3.75:1 on surface-900, below
/// the 4.5:1 text minimum, so it is never used for text.
pub(crate) const SUBTLE: Color32 = hex(0x64748B);
/// surface-800: separators.
pub(crate) const LINE: Color32 = hex(0x1E293B);
/// surface-700: control and panel outlines.
pub(crate) const LINE_STRONG: Color32 = hex(0x334155);
/// accent-glow: outlines, active-chip border, focus ring.
pub(crate) const GLOW: Color32 = hex(0xEF4444);
/// accent-active: solid fills such as progress and the selection bar.
pub(crate) const ACID_STRONG: Color32 = hex(0xDC2626);
/// accent-glow lightened (red-400) for accent text; 5.7:1 or better on every
/// surface it sits on, where the raw glow red fails on surface-800.
pub(crate) const ACID: Color32 = hex(0xF87171);
pub(crate) const SELECTED: Color32 = hex(0x1E293B);
/// accent-warn, and accent-warn at 16% over surface-900.
pub(crate) const WARN: Color32 = hex(0xF59E0B);
pub(crate) const WARN_DIM: Color32 = hex(0x342D25);
/// accent-danger.
pub(crate) const ERROR: Color32 = hex(0xEF4444);
/// Ready and success dots (spec §36.4.1).
pub(crate) const GREEN: Color32 = hex(0x22C55E);
/// accent-audio: audio files, device activity. Outlines and dots only; as
/// text it is 4.22:1 on surface-900.
pub(crate) const VIOLET: Color32 = hex(0x8B5CF6);

/// Radius scale (spec §36.3.2): small buttons 4, controls 6, panels 8, floating 12.
pub(crate) const RADIUS_CONTROL: u8 = 6;
pub(crate) const RADIUS_PANEL: u8 = 8;
pub(crate) const RADIUS_FLOAT: u8 = 12;

/// Type scale (spec §36.2.2): micro labels 10, clip text 11, standard UI 12.
pub(crate) const MICRO: f32 = 10.0;
pub(crate) const CAPTION: f32 = 11.0;
pub(crate) const SMALL: f32 = 12.0;

/// Uppercase micro-label with the spec's ~0.08em tracking. egui has no
/// letter-spacing control, so a hair space goes between letters.
pub(crate) fn tracked(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 4);
    let mut after_space = true;
    for ch in text.to_uppercase().chars() {
        if !after_space && !ch.is_whitespace() {
            out.push('\u{200A}');
        }
        after_space = ch.is_whitespace();
        out.push(ch);
        if ch == ' ' {
            // Widen word gaps so they stay visible next to tracked letters.
            out.push('\u{2009}');
        }
    }
    out
}

pub(crate) fn configure(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    for (name, bytes) in [
        ("neutra_inter", include_bytes!("../../assets/fonts/Inter-Variable.ttf.zst").as_slice()),
        ("neutra_robotomono", include_bytes!("../../assets/fonts/RobotoMono-Variable.ttf.zst").as_slice()),
        ("neutra_sans", include_bytes!("../../assets/fonts/NotoSans-Regular.ttf.zst").as_slice()),
        ("neutra_mono", include_bytes!("../../assets/fonts/NotoSansMono-Regular.ttf.zst").as_slice()),
        ("neutra_arabic", include_bytes!("../../assets/fonts/NotoSansArabic-Regular.ttf.zst").as_slice()),
        ("neutra_devanagari", include_bytes!("../../assets/fonts/NotoSansDevanagari-Regular.ttf.zst").as_slice()),
        ("neutra_cjk", include_bytes!("../../assets/fonts/NotoSansCJK-Regular.ttc.zst").as_slice()),
        ("neutra_symbols", include_bytes!("../../assets/fonts/NotoSansSymbols-Regular.ttf.zst").as_slice()),
        ("neutra_symbols2", include_bytes!("../../assets/fonts/NotoSansSymbols2-Regular.ttf.zst").as_slice()),
    ] {
        fonts
            .font_data
            .insert(name.to_owned(), Arc::new(FontData::from_owned(load_font(bytes))));
    }
    // Noto stays behind Inter and Roboto Mono for scripts and symbols they lack.
    let fallback = ["neutra_sans", "neutra_arabic", "neutra_devanagari", "neutra_cjk", "neutra_symbols", "neutra_symbols2"];
    let proportional: Vec<String> = ["neutra_inter"].into_iter().chain(fallback).map(str::to_owned).collect();
    let monospace: Vec<String> = ["neutra_robotomono", "neutra_mono"].into_iter().chain(fallback).map(str::to_owned).collect();
    fonts.families.insert(FontFamily::Name("Neutra Sans".into()), proportional.clone());
    fonts.families.insert(FontFamily::Name("Neutra Mono".into()), monospace.clone());
    for (family, names) in [(FontFamily::Proportional, proportional), (FontFamily::Monospace, monospace)] {
        let list = fonts.families.entry(family).or_default();
        for name in names.into_iter().rev() {
            list.insert(0, name);
        }
    }
    ctx.set_fonts(fonts);

    Theme::dark().store(ctx);
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = CANVAS;
    visuals.window_fill = SURFACE;
    visuals.window_stroke = Stroke::new(1.0_f32, LINE_STRONG);
    visuals.extreme_bg_color = BLACK;
    visuals.faint_bg_color = SURFACE;
    // Selection fill plus the 2px accent-glow ring at 70% used for keyboard focus.
    visuals.selection.bg_fill = SELECTED;
    visuals.selection.stroke = Stroke::new(2.0_f32, GLOW.gamma_multiply(0.7));
    visuals.widgets.noninteractive.bg_fill = SURFACE;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, LINE);
    visuals.widgets.inactive.bg_fill = RAISED;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, LINE_STRONG);
    visuals.widgets.hovered.bg_fill = HOVER;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, SUBTLE);
    visuals.widgets.active.bg_fill = ACTIVE;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, GLOW);
    visuals.widgets.open.bg_fill = ACTIVE;
    visuals.widgets.open.bg_stroke = Stroke::new(1.0_f32, GLOW);
    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = RADIUS_CONTROL.into();
    }
    visuals.override_text_color = Some(TEXT);
    visuals.window_corner_radius = RADIUS_FLOAT.into();
    visuals.menu_corner_radius = RADIUS_PANEL.into();
    // shadow-panel for popups, a deeper drop for floating windows (spec §36.5).
    visuals.popup_shadow = egui::epaint::Shadow { offset: [0, 4], blur: 6, spread: 0, color: Color32::from_black_alpha(77) };
    visuals.window_shadow = egui::epaint::Shadow { offset: [0, 18], blur: 40, spread: 0, color: Color32::from_black_alpha(140) };
    ctx.set_visuals(visuals);

    let sans_family = FontFamily::Name("Neutra Sans".into());
    let mut style = (*ctx.global_style()).clone();
    style.spacing.item_spacing = Vec2::new(6.0, 4.0);
    style.spacing.button_padding = Vec2::new(8.0, 4.0);
    style.spacing.interact_size = Vec2::new(30.0, 30.0);
    style.spacing.menu_margin = Margin::same(8);
    // 10px scrollbars, always visible (spec §36.4.8).
    style.spacing.scroll = egui::style::ScrollStyle { bar_width: 10.0, ..egui::style::ScrollStyle::solid() };
    let sizes = [
        (TextStyle::Small, CAPTION),
        (TextStyle::Body, SMALL),
        (TextStyle::Button, SMALL),
        (TextStyle::Heading, 14.0),
    ];
    for (text_style, size) in sizes {
        style.text_styles.insert(text_style, FontId::new(size, sans_family.clone()));
    }
    style
        .text_styles
        .insert(TextStyle::Monospace, FontId::new(SMALL, FontFamily::Name("Neutra Mono".into())));
    style.visuals = ctx.global_style().visuals.clone();
    ctx.set_global_style(style);
}

fn load_font(compressed: &'static [u8]) -> Vec<u8> {
    zstd::stream::decode_all(compressed).expect("embedded font must decompress")
}

#[cfg(test)]
mod tests {
    use super::tracked;

    #[test]
    fn tracked_uppercases_and_spaces_letters_but_not_words() {
        assert_eq!(tracked("ab c"), "A\u{200A}B \u{2009}C");
    }
}
