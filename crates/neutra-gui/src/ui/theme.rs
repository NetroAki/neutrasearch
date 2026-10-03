//! Design tokens and the egui theme. Values come from DESIGN.md (the Plugin UI
//! Design System sheet and Neutraudio spec §36); change them there first.

use super::*;

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// Background: window well, search box, text-edit fill.
pub(crate) const BLACK: Color32 = hex(0x0B0F14);
/// Surface 1: panels, results canvas, status bar.
pub(crate) const CANVAS: Color32 = hex(0x11171F);
pub(crate) const SURFACE: Color32 = hex(0x11171F);
/// Surface 2: raised controls, inputs, table header, cards.
pub(crate) const RAISED: Color32 = hex(0x1A222C);
/// Surface 3: hover.
pub(crate) const HOVER: Color32 = hex(0x273241);
/// Surface Elevated: pressed and open states.
pub(crate) const ACTIVE: Color32 = hex(0x324054);
/// Text Primary.
pub(crate) const TEXT: Color32 = hex(0xE6EDF4);
/// Text Secondary: metadata and labels (7.05:1 on Surface 1).
pub(crate) const MUTED: Color32 = hex(0x98A3B3);
/// Text Muted: icons and decoration only. It is 3.78:1 on Surface 1, below
/// the 4.5:1 text minimum, so it is never used for text.
pub(crate) const SUBTLE: Color32 = hex(0x64748B);
/// Divider.
pub(crate) const LINE: Color32 = hex(0x1F2A37);
/// Border.
pub(crate) const LINE_STRONG: Color32 = hex(0x2A3646);
/// Primary Hover: links, match highlight, active tab text.
pub(crate) const ACID: Color32 = hex(0x60A5FA);
/// Primary: selection, progress, focus ring.
pub(crate) const ACID_STRONG: Color32 = hex(0x3B82F6);
pub(crate) const BLUE: Color32 = hex(0x60A5FA);
/// Primary at 30% over Surface 1: selected rows and list items.
pub(crate) const SELECTED: Color32 = hex(0x1E3760);
/// Warning, and Warning at 16% over Surface 1.
pub(crate) const WARN: Color32 = hex(0xF59E0B);
pub(crate) const WARN_DIM: Color32 = hex(0x362D1C);
/// Danger.
pub(crate) const ERROR: Color32 = hex(0xEF4444);
/// Success.
pub(crate) const GREEN: Color32 = hex(0x22C55E);
/// Accent (violet): audio files.
pub(crate) const VIOLET: Color32 = hex(0x8B5CF6);
/// Info: images and video.
pub(crate) const INFO: Color32 = hex(0x38BDF8);
/// Secondary (teal): folders.
pub(crate) const TEAL: Color32 = hex(0x14B8A6);

/// Radius scale from the sheet: controls 4, cards and panels 8, floating 12.
pub(crate) const RADIUS_CONTROL: u8 = 4;
pub(crate) const RADIUS_PANEL: u8 = 8;
pub(crate) const RADIUS_FLOAT: u8 = 12;

/// Font sizes: caption 11, body small 12, body 14, subsection 18.
pub(crate) const CAPTION: f32 = 11.0;
pub(crate) const SMALL: f32 = 12.0;
pub(crate) const BODY: f32 = 14.0;

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
    // Inter and Roboto Mono lead; Noto stays behind them for scripts and
    // symbols they do not cover.
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
    let radius = RADIUS_CONTROL;
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = CANVAS;
    visuals.window_fill = CANVAS;
    visuals.window_stroke = Stroke::new(1.0_f32, LINE_STRONG);
    visuals.extreme_bg_color = BLACK;
    visuals.faint_bg_color = RAISED;
    // Selection fill plus the 2px Primary ring at 70% used for keyboard focus.
    visuals.selection.bg_fill = SELECTED;
    visuals.selection.stroke = Stroke::new(2.0_f32, Color32::from_rgba_unmultiplied(0x3B, 0x82, 0xF6, 179));
    visuals.widgets.noninteractive.bg_fill = CANVAS;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, LINE);
    visuals.widgets.inactive.bg_fill = RAISED;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, LINE_STRONG);
    visuals.widgets.hovered.bg_fill = HOVER;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, LINE_STRONG);
    visuals.widgets.active.bg_fill = ACTIVE;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, ACID_STRONG);
    visuals.widgets.open.bg_fill = ACTIVE;
    visuals.widgets.open.bg_stroke = Stroke::new(1.0_f32, ACID_STRONG);
    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = radius.into();
    }
    visuals.override_text_color = Some(TEXT);
    visuals.window_corner_radius = RADIUS_FLOAT.into();
    visuals.menu_corner_radius = RADIUS_PANEL.into();
    // Shadow scale: medium for popups, large for windows.
    visuals.popup_shadow = egui::epaint::Shadow { offset: [0, 4], blur: 12, spread: 0, color: Color32::from_black_alpha(64) };
    visuals.window_shadow = egui::epaint::Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(89) };
    ctx.set_visuals(visuals);

    let sans_family = FontFamily::Name("Neutra Sans".into());
    let mut style = (*ctx.global_style()).clone();
    style.spacing.item_spacing = Vec2::new(8.0, 4.0);
    style.spacing.button_padding = Vec2::new(8.0, 4.0);
    style.spacing.interact_size = Vec2::new(30.0, 30.0);
    style.spacing.menu_margin = Margin::same(8);
    // 10px scrollbars, always visible.
    style.spacing.scroll = egui::style::ScrollStyle { bar_width: 10.0, ..egui::style::ScrollStyle::solid() };
    let sizes = [
        (TextStyle::Small, CAPTION),
        (TextStyle::Body, SMALL),
        (TextStyle::Button, SMALL),
        (TextStyle::Heading, 18.0),
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
