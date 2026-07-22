# UI Automation Walkthrough

Date: 2026-07-21
Platform: KDE Plasma / native Wayland
Installed build: local release installation, helper build 7 / protocol 6

## Intent

Use Neutrasearch through its real installed GUI as a first-time user would. The expected basic path is:

1. Open the app.
2. Pick one or more folders.
3. Scan once.
4. Type a filename and get useful results immediately.

Advanced controls should remain available without competing with that path.

## Test setup

A small user-owned fixture was created at `/home/netroaki/NeutrasearchUXTest` with nested folders and representative PDF, DOC-like, image, Markdown, and text names. The real KDE folder picker, native Wayland window, keyboard navigation, clipboard, first-run scan action, reference result set, menus, and attempted resize interaction were exercised with `ydotool`, KWin, and Spectacle.

No password was entered or automated. No unrestricted directory walk was run. A synthetic built-in reference index was used to review populated search interactions after the real first scan failed.

## Chronological user thoughts

### 1. First launch

Evidence: `/tmp/neutrasearch-walkthrough-03-tab.png`

The first screen is much better than the earlier version. It is calm, short, and understandable. There is one sentence-sized task, one empty folder area, `+ Add folder`, and a disabled `Scan` button. The logo and window identity are correct.

Immediate thought: **“I know what to do.”**

The remaining top-level File/Search/View/Help menus are not harmful, but they are unnecessary during onboarding. Hiding or disabling them until setup completes would make the task even more self-contained.

### 2. Adding a folder

Evidence:

- `/tmp/neutrasearch-walkthrough-04-picker.png`
- `/tmp/neutrasearch-walkthrough-09-home-selected.png`
- `/tmp/neutrasearch-walkthrough-10-folder-added.png`

The app opens the native KDE folder picker with the correct title, `Add search folder`. This is the right choice. It behaves like the rest of the desktop and supports locations, devices, and network entries without Neutrasearch recreating a file browser.

After choosing a folder, the onboarding screen shows the full path and a quiet Remove action. `Scan` becomes enabled. This is clear and appropriately minimal.

Keyboard friction was noticeable: focus starts in the menu bar, not on the primary task. Reaching the folder/scan actions requires traversing menus first, and the focus ring is too subtle to tell whether Add or Scan is selected. During automation, Enter reopened the folder picker when Scan was expected. A first-run screen should focus `+ Add folder` when empty and `Scan` immediately after a folder returns.

### 3. First scan

Evidence: `/tmp/neutrasearch-walkthrough-14-real-scan-attempt.png`

This is the critical failure.

The selected folder was accepted, but the first scan did not show an administrator prompt or produce an index. The app immediately left onboarding and opened the full search interface with:

- `A native location is unavailable`
- `Review scanner access before building the first index.`
- `Index details`
- `Retry as administrator`
- zero results

Immediate thought: **“I did the only thing the app asked and it did not work.”**

The failure also exposes the most complex version of the interface at the exact moment a basic user needs the least complexity. The setup should remain in a compact recovery state with one primary action such as `Allow access and scan`, plus a small disclosure for details. It should not drop a first-time user into an empty power-user search workspace.

The app persisted onboarding as complete before a successful first index. Relaunching therefore does not return to the simple setup screen. That follows the earlier “never show it again” request literally, but in practice failed setup needs a simple recovery surface until the first usable index exists.

### 4. Populated search experience

Evidence:

- `/tmp/neutrasearch-walkthrough-21-reference-results.png`
- `/tmp/neutrasearch-walkthrough-22-search.png`

The built-in reference index was used after the real scan failed.

Search itself is excellent. Typing `invoice-2026` reduced the visible set immediately from 52 matches to 16, with reported query times around 15–17 microseconds. The dense details table is readable, the filename/path separation is strong, type chips scan quickly, and modified time/size are easy to compare.

Immediate thought: **“This is the actual product; get me here with less surrounding UI.”**

The selected row is visible without becoming loud. The default Details presentation is a good desktop default.

### 5. Basic surface versus advanced controls

In the populated state, the always-visible chrome includes:

- Everywhere scope
- Match Name + path
- Case
- Regex
- settings icon
- All / Files / Folders
- Details / List / Grid / Treemap
- Ready / index current status
- indexed-object count
- keyboard shortcut legend
- Ready / Diagnostics footer controls

All of these are individually defensible, but together they make search look more complicated than it is.

The basic surface only needs:

1. Search field
2. Results
3. A small file/folder filter
4. One unobtrusive overflow/settings affordance

Recommended progressive disclosure:

- Move scope, Name-versus-path, Case, and Regex into the Search menu or a single filter popover.
- Hide scope entirely when only one root is configured.
- Keep All / Files / Folders visible if desired; they are understandable and frequently useful.
- Move the four view choices into View. Show only the active view plus one dropdown/icon in the results strip.
- Move keyboard hints to Help or show them contextually on first use.
- Remove `Ready · index current` and the duplicate bottom-right `Ready`; absence of an error is enough.
- Keep diagnostics, network helpers, native lane details, cache paths, privilege actions, and rebuild controls in File/settings.

This is **not** a Basic/Advanced mode. It is one interface with sensible defaults and deeper controls placed where users expect them.

### 6. Menus

Evidence: `/tmp/neutrasearch-walkthrough-26-file-menu.png`

The File menu is appropriately short:

- Locations and settings
- Rebuild index
- Copy selected path
- Exit

This is a good home for infrequent and advanced operations. Search-specific toggles and view selection should follow the same pattern instead of also occupying permanent horizontal space.

### 7. Clipboard and keyboard behavior

Search text entry and live filtering worked. However, the footer says `Up/Down Select` and `Ctrl+C Copy` while the search field commonly retains focus. In that state, arrow keys belong to the text editor and Ctrl+C does not copy the highlighted result path. Clipboard contents remained unchanged during the keyboard-only result-copy attempt.

The UI should either:

- use unambiguous result shortcuts that work while search remains focused, such as Ctrl+Up/Ctrl+Down and Ctrl+Shift+C; or
- reliably transfer focus to results on Down and make Escape return to search; or
- stop advertising shortcuts that are contextually unavailable.

Mouse users can still use row/context actions, but keyboard behavior should match the visible legend.

### 8. Resize attempt

A KWin keyboard-resize attempt was inconclusive because focus moved between desktop applications during injected input. Internal app layout remained stable in captured states. No claim is made here about whether pointer-driven outer-window resize latency is fully resolved.

## Severity-ranked findings

### Blocker

1. **The real first scan failed after the user completed the only required setup task.** No authorization prompt appeared, no index was created, and the app moved to a generic unavailable-location state.

### High

2. **Failed first setup reveals the entire advanced interface instead of a simple recovery path.**
3. **Onboarding is marked complete before the first usable index exists.** The user cannot naturally return to the simple setup workflow after failure.
4. **Too many search and view controls are permanently visible.** The excellent core search looks more complicated than it is.

### Medium

5. **Keyboard focus order prioritizes menus over the onboarding action.**
6. **The focus indication is too subtle to distinguish Add folder from Scan.**
7. **The advertised result-selection/copy shortcuts conflict with retained search-field focus.**
8. **Status is duplicated at the top and bottom, while the footer exposes expert-oriented diagnostics and shortcut text.**

### Good as-is

- Correct native icon and in-app logo
- Sullen slate/periwinkle visual direction
- Native folder picker
- Minimal folder list with Remove
- Fast live search response
- Dense Details table
- File menu organization
- Advanced diagnostics existing behind settings rather than being removed

## Recommended default layout

```text
┌ Neutrasearch ───────────────────────────────────────── [⋯] ┐
│ 🔍 Search files and folders…                              │
│ [All] [Files] [Folders]                         [Details ▾]│
├───────────────────────────────────────────────────────────┤
│ Name                     Path               Modified Size │
│ …results…                                                │
└───────────────────────────────────────────────────────────┘
```

The overflow/menu layer should contain location management, rebuild, scope, match target, case, regex, view variants, network helpers, diagnostics, and support links.

## Bottom line

The first screen now communicates the product simply, and the populated result table proves the core interaction can feel fast and professional. The next UX pass should focus on two things only:

1. Make the first scan genuinely succeed—or keep failure recovery as simple as setup.
2. Let search and results dominate; move expert controls into existing menus and popovers.
