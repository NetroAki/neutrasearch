# Follow-up UI/UX Automation Walkthrough

Date: 2026-07-22
Platform: KDE Plasma / native Wayland
Installed build: local release build, helper build 8 / protocol 7

## Scope

This walkthrough repeated the first-time and populated-search workflows after the UX and scan-launch fixes. It explicitly asked:

1. Are there too many options exposed for non-technical users?
2. Do advanced users have easy access to the tools they will want?
3. Will this be a simple workflow and “just work” for non-technical users?
4. Is there additional UI/UX clutter we do not need?
5. How can the workflow be simplified for non-technical users without limiting advanced users?

Automation used the installed release binary, KDE’s real folder picker and authorization agent, `ydotool`, KWin scripting, Spectacle, and a built-in populated reference index. A small fixture was selected at `/home/netroaki/NeutrasearchUXWalkthrough2`.

No password was entered or automated. The authorization dialog was deliberately cancelled, so this walkthrough verifies that the real desktop authorization request now appears and that cancellation recovers cleanly; it does not claim a password-approved native scan completed.

## Evidence

- `/tmp/neutrasearch-ux2-01-first-run.png` — clean first launch
- `/tmp/neutrasearch-ux2-picker-selected.png` — native picker at the fixture
- `/tmp/neutrasearch-ux2-02-folder-added.png` — selected folder and focused scan action
- `/tmp/neutrasearch-ux2-06-auth-prompt.png` — real KDE administrator authorization prompt
- `/tmp/neutrasearch-ux2-07-auth-cancel-recovery.png` — compact recovery after cancellation
- `/tmp/neutrasearch-ux2-08-installed-search.png` — installed populated search workspace
- `/tmp/neutrasearch-ux2-selection-check.png` — keyboard-selected result while search remains usable
- `/tmp/neutrasearch-ux2-11-search-menu.png` — advanced search tools in the Search menu
- `/tmp/neutrasearch-ux2-12-resized.png` — compact 800×520 layout
- `/tmp/neutrasearch-ux2-14-no-locations.png` — dedicated recovery after all locations are removed

## Walkthrough thoughts

### 1. First launch

The first screen now contains only the product name, `Choose where to search`, an empty location area, `Add folder`, and a disabled `Allow access and scan` action. File/Search/View/Help are hidden during setup.

Immediate thought: **“There is one task and I know where to begin.”**

The focused Add action has an obvious border. After the native picker returns, focus moves to `Allow access and scan`, so keyboard users do not have to rediscover the next step.

### 2. Folder selection

The KDE picker opens with the concrete title `Add search folder`. Selecting the fixture returns to the same setup surface, showing the exact folder and a quiet Remove action.

Immediate thought: **“The app remembered what I chose; the next button explains why access is needed.”**

### 3. Administrator approval

Choosing `Allow access and scan` from a real desktop-launched process now opens KDE’s native `Authentication Required` dialog for the trusted system helper. This directly fixes the previous walkthrough’s central failure, where no prompt appeared.

The test cancelled rather than entering a password. Neutrasearch returned to setup instead of opening an empty expert workspace. It retained the folder, stated that no existing index changed, and offered `Allow access and scan again` plus optional Details.

Immediate thought: **“Cancellation is recoverable and I have not lost my setup.”**

The persisted settings correctly remained `onboarding_complete: false` after failure. Setup will not disappear until a native lane reports a usable index.

### 4. Populated search

The installed release build’s populated reference view is substantially calmer:

- one large search field
- result count
- All / Files / Folders
- one active-view dropdown
- results

Scope, match target, case sensitivity, regex, diagnostics, rebuild, and alternate views no longer compete with the main task. Duplicate Ready indicators, the footer, timing prose, shortcut legend, and four permanently visible view buttons are gone.

Immediate thought: **“This now looks like a file searcher rather than a scanner control panel.”**

Live search remained immediate. At 800×520 the table stayed readable and controls did not collide.

### 5. Keyboard workflow

`Ctrl+Up/Down` selected results while avoiding conflict with cursor movement in the search field. `Ctrl+Insert` copied the selected full path to the real KDE clipboard; the observed value was:

`/home/alex/Documents/Accounts/Invoice - Acme - June.pdf`

The File and Help menus advertise the same shortcut. This is less familiar than Ctrl+C, but it is unambiguous and works while search retains focus.

### 6. Advanced tools

The Search menu exposes:

- Focus search
- Clear search
- Match: Name / Name + path / Path only
- Case sensitive
- Regular expression
- result-selection shortcut help

File contains locations/settings, rebuild, selected-path copy, and exit. View contains Details, List, Grid, and Treemap. Settings/diagnostics retains native-lane details, administrator rebuild, durable-index information, and network-server controls.

Immediate thought: **“Experts can reach everything in one predictable menu without making everyone else understand it.”**

Non-default search modifiers also have compact removable indicators in the results toolbar, preventing hidden Regex/Case/Path/scope state from making results appear mysteriously wrong.

### 7. No locations

A completed user who removes every location now sees `No folders selected` and one `Add folder` action, not the inaccurate `No matching objects` empty state. Adding a folder from this recovery surface proceeds directly into the scan flow.

## Answers to the five UX questions

### Are there too many options exposed for non-technical users?

**No on the default path.** Setup has one task. The ready workspace exposes search, results, a simple kind filter, and one view selector. The remaining menu names are standard desktop concepts rather than technical scanner vocabulary.

The settings dialog is intentionally technical, but it is no longer part of the basic workflow.

### Do advanced users have easy access to the tools they will want?

**Yes.** File/Search/View provide one-click or one-keyboard-navigation access to location management, rebuild, copy, match semantics, scope, case, regex, sorting through the table header, alternate visualizations, diagnostics, administrator retry, and network support.

The advanced controls are hidden by default, not removed.

### Will this be a simple workflow and “just work” for non-technical users?

**The workflow is now simple and the authorization handoff works correctly.** The observed path was:

1. Add folder.
2. Allow access and scan.
3. Approve the native OS dialog.
4. Search.

Steps 1–2 and the real authorization dialog were verified. Cancellation recovery was verified. A password-approved scan was not completed in automation, so full successful first-index proof still requires a human-authorized run.

### Is there additional UI/UX clutter we do not need?

**No major default-surface clutter remains.** The remaining possible refinement is inside `Settings and diagnostics`: visually separate everyday Locations from technical Index/Native lanes/Network sections more strongly. That is a medium-priority organization improvement, not a blocker.

The `Ctrl K` hint inside the search field is helpful but optional; it could disappear after first use if an even quieter surface is desired.

### How can we simplify further without limiting advanced users?

Continue the current progressive-disclosure model:

- keep default search controls minimal
- show only active non-default modifiers
- keep detailed scanner state behind File → Locations and settings
- keep recovery cause-specific and action-first
- never introduce separate Basic/Advanced modes
- add a small Settings section hierarchy rather than moving expert controls back onto the main screen

## Security and reliability changes supporting the UX

- Protocol v7 sends approved roots to the privileged helper.
- The helper validates roots against trusted OS mount metadata and filters records before returning them to the user process.
- Scan roots are snapshotted for the active scan, and location editing is disabled during scan/cache publication.
- Unexpected helper EOF always terminates the scanning state.
- Windows settings publication now replaces an existing file correctly; repeated-save tests cover the transition from incomplete to complete onboarding.

## Remaining finding

### Human confirmation required

A person must approve the KDE password dialog once to verify that this machine’s Btrfs native lane completes and publishes the selected fixture. Automation correctly did not enter or store a password.

## Bottom line

The product now has a genuinely simple default shape: **choose a folder, approve once, search**. Advanced functionality remains close by in conventional menus, while first-run errors and no-location states recover without exposing the scanner control surface. The previous blocker—no visible authorization prompt—was fixed and reproduced successfully through the real desktop launch path.
