# UI/UX Walkthrough, Fix Pass, and Rewalk

Date: 2026-07-22
Application: Neutrasearch desktop GUI
Build: local release, protocol 7 / helper build 8

## Requested questions

This review explicitly asked:

1. Are there too many options exposed for non-technical users?
2. Do advanced users have easy access to the tools they will want?
3. Will this be a simple workflow and “just work” for non-technical users?
4. Is there additional UI/UX clutter we do not need?
5. How can the workflow be simplified for a non-technical user without limiting advanced users?

## Method and limits

The baseline walkthrough used the installed release under KDE Plasma/Wayland and exercised first run, ready search, selection, menus, sorting, settings, and error states. The implementation was also reviewed directly, with a separate designer review and a local egui UI probe.

After the fixes, the ready/selection states were rendered under the live KDE desktop. The desktop session ended before the last two captures, so the final Settings and invalid-regex states were rendered from the same installed binary in a local 1280×800 Xvfb session. No external service received screenshots or product data.

No password was entered or automated. After the walkthrough, the user approved a narrowly scoped local polkit rule for the root-owned Neutrasearch helper and an exact bounded scan of `/home/netroaki/NeutrasearchUXFinalFixture`. That follow-up completed the first native index and restart proof without storing a password.

## Baseline findings

### Main workflow

The main path was already appropriately quiet:

- first run showed one folder-selection task
- the ready workspace centered search and results
- ordinary users did not have to understand scanner lanes, regex, or native filesystems

Verdict: **not too many options on the default path**.

### Advanced access

File, Search, and View menus exposed location management, rebuild, match target, folder scope, case sensitivity, regex, and alternate views.

Verdict: **advanced tools were reachable**, but result actions and sorting required too much prior knowledge.

### Remaining friction

1. `Settings and diagnostics` mixed folders, index internals, native lanes, administrator rebuild, and network watching in one flat surface.
2. The Modified table header secretly cycled through every sort mode; Name, Path, and Size did not behave like normal sortable headers.
3. Open/reveal/copy actions depended on double-click, right-click, or knowing `Ctrl+Insert`.
4. Invalid regex syntax appeared to be a valid search with zero results.
5. Adding or removing folders after onboarding saved the choice but did not immediately refresh the index.
6. Non-ready state was duplicated in both a top status chip and a larger action banner.
7. No-results recovery explained the problem but did not provide direct reset actions.

## Fixes applied

### Progressive settings hierarchy

The dialog is now titled **Locations and index**. Everyday folder controls appear first. Technical content is grouped behind collapsible sections:

- Index status
- Scanner details
- Index maintenance
- Network folders

Labels were made more concrete:

- Objects → Indexed items
- Search generation → Index generation
- Durable index → Saved index location

Administrator rebuild and network discovery remain available without confronting ordinary users immediately.

### Conventional sorting

Every details-table header is now directly clickable:

- Name ↑
- Path ↑
- Modified ↓
- Size ↓

The active sort is identified with text and an arrow, not color alone. Hover text explains each target.

### Selected-result actions

Selecting a result now exposes a **Selected** menu beside the view selector with:

- Open
- Show in folder
- Copy full path

Double-click, context-menu actions, File-menu copy, and `Ctrl+Insert` remain available as expert accelerators.

### Actionable query recovery

An invalid expression now shows **Invalid regular expression**, not `No matching objects`. It offers:

- Clear search
- Reset search options

A valid search with no matches remains a distinct state. Active Regex/Case/match/scope modifiers remain visible as removable indicators.

### Automatic index refresh after location changes

After onboarding, adding or removing a folder now starts the appropriate native rebuild automatically. The user no longer has to infer that a second Rebuild action is required.

### Reduced duplicate status

The redundant top-bar non-ready chip was removed. Permission, stale-index, and background-indexing states use one action-first banner.

### Recovery wording

Generic `Details` wording was replaced with concrete actions such as **Review scan details** and **Review folders and access**.

## Post-fix walkthrough

### First-run workflow

The workflow remains:

1. Add folder.
2. Allow access and scan.
3. Approve the operating-system authorization dialog.
4. Search.

Setup still hides File/Search/View/Help and remains active until a usable first index is published. Cancelling authorization retains folder choices and offers a focused retry.

Assessment: **simple, action-first, and understandable for a non-technical user**.

### Ready search

Search remains the dominant control. The toolbar exposes only result count, All/Files/Folders, selected-result actions when relevant, and the current view. Sort affordances now follow desktop-table conventions.

Assessment: **no meaningful default-surface clutter remains**.

### Native first scan and restart

The final bounded walkthrough selected `/home/netroaki/NeutrasearchUXFinalFixture` on the Btrfs `/home` filesystem and activated **Allow access and scan**. The scoped polkit rule launched only `/usr/local/lib/neutrasearch/neutrasearch-helper`; no password prompt or password file was used.

The initial screen advanced through **Building the first index** and published exactly six selected-root records: the fixture root, two directories, and three files. Settings changed to `onboarding_complete: true`, and a 2,055-byte durable `index.nsx` was published in the isolated walkthrough data directory.

The GUI was then terminated by its approved 120-second outer timeout and restarted with the same settings/data directories. Searching `Invoice` immediately returned only `Walkthrough Invoice 2026.txt`, proving durable index restoration and post-restart querying. No path outside the approved fixture appeared in the index.

Assessment: **the previously unverified first-index success path is now proven for the approved Btrfs fixture**.

### Settings

Opening Locations and index now shows the selected folders immediately. Four compact collapsed rows preserve the full maintenance/diagnostic toolset without making it part of ordinary folder management.

Assessment: **the previous concentration of technical options is resolved**.

### Advanced workflow

Advanced users retain:

- match-by-name/name-and-path/path
- folder scope
- case-sensitive and regex search
- four result views
- direct per-column sorting
- rebuild and administrator rebuild
- scanner details and saved index path
- network-server watching
- keyboard selection, open, and copy actions

Assessment: **progressive disclosure did not remove expert capability and made several expert operations faster**.

### Error and empty states

Invalid regex, valid no-match, no-folders, permission failure, stale index, and first-index progress now have distinct language and appropriate next actions.

Assessment: **a non-technical user is less likely to interpret a configuration error as “the file does not exist.”**

## Answers after fixes

### Are there too many options exposed for non-technical users?

**No.** First run has one task, ready search has a compact toolbar, and folder management presents folders before technical details.

### Do advanced users have easy access to the tools they will want?

**Yes.** Advanced search, views, sorting, file actions, scanner details, rebuild modes, and network tools remain within one menu or one expandable section.

### Will this be a simple workflow and “just work” for non-technical users?

**Yes for the tested local Btrfs fixture.** Folder changes rebuild automatically, authorization cancellation is recoverable, query mistakes provide direct recovery, and the native first index survived restart. Cross-platform and network-server flows still require their own platform-specific testing.

### Is there additional UI/UX clutter we do not need?

**No major clutter remains.** The `Ctrl K` hint is optional but small and useful. Diagnostics remain intentionally dense only after a user expands them.

### How can we simplify without limiting advanced users?

Continue the established pattern:

- default to search and results
- expose active non-default state rather than every possible option
- use conventional menus and table interactions
- keep diagnostics collapsed, not removed
- make failures action-first
- avoid separate Basic/Advanced modes

## Evidence

Baseline and ready-state evidence:

- `/tmp/neutrasearch-ux3-baseline.png`
- `/tmp/neutrasearch-ux3-ready-baseline.png`
- `/tmp/neutrasearch-ux3-settings-baseline.png`

Post-fix evidence:

- `/tmp/neutrasearch-ux3-after-ready.png`
- `/tmp/neutrasearch-ux3-after-selected.png`
- `/tmp/neutrasearch-ux3-after-settings-x11.png`
- `/tmp/neutrasearch-ux3-after-invalid-regex-final.png`
- `/tmp/neutrasearch-final-01-before-scan.png`
- `/tmp/neutrasearch-final-02-scan-started.png`
- `/tmp/neutrasearch-final-03-scan-complete.png`
- `/tmp/neutrasearch-final-05-restart-search.png`

Earlier real authorization evidence remains:

- `/tmp/neutrasearch-ux2-06-auth-prompt.png`
- `/tmp/neutrasearch-ux2-07-auth-cancel-recovery.png`

## Validation

Passed:

- `cargo fmt --all -- --check`
- `cargo check --workspace`
- `cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `npm test --prefix packages/pi-neutrasearch` (14/14)
- `python scripts/package_release.py self-test`
- `bash scripts/check-no-walk.sh`
- release build for GUI and helper
- final GUI-only tests, Clippy, and release rebuild after the last visual-alignment adjustment

The updated GUI and helper were installed root-owned with mode 0755.

## Remaining verification scope

The Linux/Btrfs first-index path is complete for the bounded fixture. Remaining verification is platform-specific rather than a blocker for this walkthrough: Windows service/index behavior, macOS Spotlight/fallback behavior, and real mounted-network-server helper installation and querying.
