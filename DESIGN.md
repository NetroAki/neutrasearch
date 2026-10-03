# DESIGN

Direction for the Neutrasearch desktop UI. It is transcribed from the design
system the project owner pointed at: the "Plugin UI Design System" sheet on the
desktop (`~/Desktop/cd2d7947...ebd65f.png`) and the Neutraudio design spec
(`neutraudio/docs/DESIGN.md` -> `docs/product-specs/06-design-system.md` §36).
Where the two disagree the sheet wins; the one disagreement is the accent
(sheet: blue primary, spec: red). Nothing here is invented direction. Lines
marked **Adaptation** are the only agent decisions, each with its reason.

Dial: ENERGY 1 / RHYTHM 1 / MOTION 1 (calm, uniform, hover-only motion). Reason:
a search utility is opened dozens of times a day; Neutraudio's own principles
ask for dense, flat pro-tool UI.

## Principles (Neutraudio §36.8)

- Dark first. Flat with depth: depth comes from borders and shadows, no glass.
- Dense and compact: small controls, micro-labels, high information density.
- Accent consistency: every interactive highlight uses the accent tokens; no ad-hoc colours.
- Basic paths stay light; advanced controls sit behind progressive disclosure.
- No hover-only critical actions; tooltips also show on keyboard focus.

## Colour tokens (sheet, section 01)

| Token | Hex | Use here |
|---|---|---|
| Background | #0B0F14 | window background, search box well |
| Surface 1 | #11171F | panels, results canvas, status bar |
| Surface 2 | #1A222C | raised controls, inputs, table header, cards |
| Surface 3 | #273241 | hover |
| Surface Elevated | #324054 | pressed, open menus |
| Text Primary | #E6EDF4 | body text |
| Text Secondary | #98A3B3 | metadata, labels, placeholders |
| Text Muted | #64748B | icons, borders of disabled controls, decoration |
| Border | #2A3646 | control and panel outlines |
| Divider | #1F2A37 | row and section separators |
| Primary | #3B82F6 | selection, active tab, progress, focus ring |
| Primary Hover | #60A5FA | links, match highlight, active tab text |
| Secondary (teal) | #14B8A6 | folders |
| Accent (violet) | #8B5CF6 | audio files |
| Info | #38BDF8 | informational tint |
| Success | #22C55E | ready, up to date |
| Warning | #F59E0B | access banner, stale |
| Danger | #EF4444 | errors, unavailable |

**Adaptation (contrast, WCAG AA):** Text Muted is 3.78:1 on Surface 1, so it is
never used for text; text that would use it uses Text Secondary (7.05:1).
White on Primary is 3.68:1, so text on a solid Primary fill is Background
(#0B0F14), not white.

**Adaptation (selection):** the sheet shows a solid blue selected row. Here the
fill is Primary at 30% over Surface 1 (#1E3760) with Text Primary on it
(10.04:1), so selected rows stay readable and calm at 10,000 rows. File-type
badges keep a coloured outline (Accent for audio, Info for images and video,
Secondary for folders) with Text Primary letters, because violet letters on
Surface 2 are only 3.79:1.

## Typography (sheet, section 02; Neutraudio §36.2)

Inter for UI text, Roboto Mono for paths, sizes, and counts. CJK, Arabic, and
Devanagari fall back to Noto as before.

| Style | Size / line | Weight |
|---|---|---|
| H3 / subsection | 18 / 24 | 600 |
| Body | 14 / 20 | 400 |
| Body small | 12 / 16 | 400 |
| Caption / label | 11 / 14 | 400 |
| Overline | 10 / 14, uppercase | 500 |

Overline is for sidebar section titles only. egui has no tracking control and
draws one weight per font file, so overline is uppercase only and "strong"
text is colour and size, not a heavier face.

## Shape (sheet, sections 03 to 06; Neutraudio §36.3)

- Radius scale: 0 / 2 / 4 / 8 / 12 / 16 / 24 / full. Controls and inputs 4, cards and panels 8, floating windows and menus 8 to 12, progress bars and status dots full.
- Spacing scale: 4 / 8 / 12 / 16 / 24 / 32 / 48 / 64.
- Strokes: 1px default outlines, 2px for focus and active tab underline.
- Shadows: small 0 1 2 / 0.2, medium 0 4 12 / 0.25 (menus, popups), large 0 8 24 / 0.35 (windows).
- Focus ring: 2px Primary at 70%.

## Components

- Tabs: text tabs with a 2px Primary underline on the active one (sheet 09).
- Selected row or list item: solid Primary-tinted fill with Text Primary (sheet 21, 22).
- Inputs and search: Surface 2 fill, 1px Border, magnifier at left (sheet 10).
- Buttons: primary solid Primary, secondary Surface 2 with Border, tertiary text-only (sheet 07).
- Table: caption-size Text Secondary header, 1px Divider rows (sheet 21).
- Context menu: Surface 1, 1px Border, radius 8, 224px minimum width (spec §36.4.7).
- Status bar: compact bar, Text Secondary labels with values (sheet 35).
- Scrollbars: 10px, Surface 3 thumb (spec §36.4.8).

## Icons

The spec names Phosphor Icons bundled as SVG. The app still draws its small
glyphs as vector shapes; bundling Phosphor is tracked in `current-Debt.md`.
