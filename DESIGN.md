# DESIGN

Direction for the Neutrasearch desktop UI: it should look like the Neutraudio
plugins and DAW shell. Transcribed from the owner's sources, not invented:

- `neutraudio/docs/DESIGN.md` -> `docs/product-specs/06-design-system.md` §36
  (colour, type, radius, shadow, component rules).
- `neutraudio/crates/neutraudio-ui/src/shell/tokens.rs` (the palette and radii
  the native egui shell actually uses).
- `neutraudio/ui-mockup/ui-mockup-current.png` (what that looks like: device
  cards, filter chips, tracked uppercase labels, recessed wells).
- The "Plugin UI Design System" sheet on the desktop is a generic component
  inventory with a blue primary. It is not the palette source; Neutraudio's
  accent is red.

Lines marked **Adaptation** are the only agent decisions, each with its reason.

Dial: ENERGY 1 / RHYTHM 1 / MOTION 1 (calm, uniform, hover-only motion).
Reason: a search utility is opened dozens of times a day, and §36.8 asks for
dense, flat pro-audio UI.

## Principles (§36.8)

- Dark first. Flat with depth: borders, recessed wells, and shadows; no glass.
- Dense and compact: small controls, micro-labels, high information density.
- Accent consistency: highlights use the accent roles, no ad-hoc colours.
- No hover-only critical actions; tooltips also show on keyboard focus.

## Colour (§36.1, tokens.rs)

| Role | Hex | Use here |
|---|---|---|
| surface-950 | #020617 | window, results canvas, recessed wells, search field |
| surface-900 | #0F172A | panels, toolbars, table header, status bar |
| surface-800 | #1E293B | raised controls, selected row, separators |
| surface-700 | #334155 | outlines, pressed |
| surface-500 | #64748B | icons and decoration |
| surface-400 | #94A3B8 | metadata, labels |
| surface-200 | #E2E8F0 | body text |
| accent-glow | #EF4444 | active outline, focus ring, danger |
| accent-active | #DC2626 | selection bar, progress fill |
| accent-audio | #8B5CF6 | audio files, activity dots |
| accent-warn | #F59E0B | warning banner, folders, archives |
| green | #22C55E | ready dots, images and video |

**Adaptation (contrast, WCAG AA):** surface-500 is 3.75:1 on surface-900, so it
is never used for text. Raw accent-glow is 3.89:1 on surface-800 and
accent-audio is 4.22:1 on surface-900, so accent text uses red-400 (#F87171,
5.3:1 or better everywhere it appears) and violet is used only for outlines
and dots.

## Type (§36.2)

Inter for UI, Roboto Mono for paths, sizes and counts (CJK, Arabic, Devanagari
fall back to Noto). Sizes: 10 micro labels, 11 captions and paths, 12 standard
UI text, 14 section headers. Micro labels are uppercase and tracked (~0.08em).

**Adaptation (tracking):** egui has no letter-spacing control, so tracked
labels put a hair space between letters (`tracked()` in `ui/theme.rs`).
Bold is colour and size, because egui draws one weight per font file.

## Shape (§36.3, §36.5)

- Radius: 4 small buttons, 6 controls and inputs, 8 panels and cards, 12 floating windows and filter chips, full for dots and bars.
- Strokes: 1px outlines, 2px focus ring (accent-glow at 70%).
- Scrollbars 10px. Rows 26px. Shadows follow §36.5.

## Components

- Filter chips (mockup browser filters): pill, uppercase micro label, 1px surface-700 outline. Active: accent-glow outline, 14% accent-glow tint, red-400 text.
- Table header: tracked uppercase micro labels on surface-900. Rows are flat on surface-950 with surface-800 dividers.
- Selected row: surface-800 fill with a 3px accent-active bar on the left, text unchanged.
- Search field: surface-950 well with a 1px outline, magnifier inside, Ctrl+K key cap.
- Side panel (device-card chrome): surface-900, 1px outline, radius 8, tracked uppercase title; location rows sit in a recessed surface-950 well.
- File-type badges: outlined in the type colour (audio violet, images and video green, folders and archives amber, PDF red, others neutral) with text-colour letters.
- Context menus and windows: surface-900, 1px outline, radius 8 to 12.
- Menu bar: transparent items on surface-950 that fill with surface-3 only on hover or while open; the wordmark is a tracked uppercase title.
- Toolbar dropdowns: transparent ghost buttons with a 1px surface-700 outline, 28px tall, level with the filter chips. The list/grid toggle is one joined segmented control with outer corners only.
- Folder tree: tracked uppercase column headers on surface-900, 26px rows, a share bar (surface-500, accent-active on the open folder), size and item counts in Roboto Mono 11. The open folder gets the same selection bar as a result row. A click opens a folder, a double-click or chevron expands it.
- Map tiles: the type colour at about a third strength as the fill, a stronger outline, Text Primary labels (never white on a saturated fill).

## Icons

The spec names Phosphor Icons bundled as SVG. The app still draws its small
glyphs as vector shapes; bundling Phosphor is tracked in `current-Debt.md`.
