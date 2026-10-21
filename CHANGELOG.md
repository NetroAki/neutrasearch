# Changelog

All notable changes are documented here. Neutrasearch follows semantic versioning; pre-1.0 releases may contain intentional compatibility breaks described in the release notes.

## [Unreleased]

## [0.1.30] - 2026-10-21

### Known limits
- The Windows scanner service accepts connections from any local user and returns metadata only. Per-caller access checks are planned.
- The Linux watch service holds CAP_SYS_ADMIN and CAP_DAC_READ_SEARCH for fanotify; it is now non-dumpable and runs with no-new-privileges.

### Fixes

- Copy and move on Linux work with coreutils 9.4 (Ubuntu 24.04). 0.1.29 required
  coreutils 9.5 and could fail to copy or move files there. Files are copied under
  a private name and renamed into place without replacing, so a destination that
  appears mid-copy is reported instead of skipped.
- Rename on Windows and macOS uses the atomic no-replace primitives, so it can no
  longer overwrite a file that appears during the operation.
- Large indexes built from several spill chunks merge in the same path order the
  chunks were sorted in. Mixed-case names on Windows and macOS, and names with a
  backslash on Linux, could previously interleave out of order.
- A macOS scan stops with an error when a listed path cannot be read; only paths
  that vanished after the Spotlight query are skipped.
- Resolve the macOS /var alias in the seed snapshot, flush staged files on Windows
  by opening them writable, and compare folded paths consistently in the search
  cache. The test suite now passes on Linux, Windows and macOS.

### Packaging and release

- Every package now ships THIRD-PARTY-NOTICES.txt with the licences of bundled
  fonts and Rust dependencies.
- Releases run the full CI workflow (tests, lints, audit, dependency review)
  before building artifacts. Dependency review also runs on pushes.
- Release tooling moved from scripts/ to packaging/tools/.
- The polkit example that allows passwordless indexing is now an opt-in example
  file, not part of any package.
- The Pi package pins its platform binaries to the matching release.

### Dependencies

- Make image tracing, text shaping and Illustrator import optional in
  egui_expressive and turn them off here. This removes 74 crates, including the
  unmaintained adler, ansi_term, atty, rustybuzz and ttf-parser, and updates
  event-listener to 5.4.2.

### Documentation

- The README lists what each platform supports: live updates and trash are
  Linux-only for now. State that MCP search covers every indexed path unless
  NEUTRASEARCH_MCP_ALLOWED_ROOTS is set, and that undo history is kept in memory.

## [0.1.29] - 2026-10-20

### Smaller disk catalog

- Replace the SQLite browse B-trees with memory-mapped flat columns plus a small live-row table. The catalog for a 50M-record index drops from 52 GB to about 1.6 GB, with the same sort orders, live updates and tombstones.
- Add a licence policy (deny.toml) and move lopdf to 0.42 through the shared egui_expressive crate.

### Quick-search shortcut

- Default quick search to Alt+K (Option+K on macOS). Add Search > Keyboard
  shortcuts to edit, add, remove, disable or reset global bindings. Persist
  choices across restarts and report conflicts while preserving active keys.
- Release retired Ctrl+K and Super+K bindings on successful KDE migration.
  Add native per-user shortcut agents for Windows and macOS; hardware/runtime
  validation on those platforms remains pending.

### Recover from native event gaps

- Request per-event errors from Linux mount fanotify readers. Files deleted
  before their saved-file event is read no longer stop indexing; later valid
  events in the batch remain available. Older kernels use a narrow vanished-file
  retry path. Permission, I/O and genuine queue-loss errors remain explicit.
- Recover Btrfs event overflow and unresolved event paths through native changed
  metadata from the saved inclusive transaction cursor. Keep the reader running
  and the base available while replay catches up, without directory walking or
  rebuilding the full index.
- Sync recovery cursors and their parent directory after atomic replacement.
  Propagate malformed cursors and failed saves rather than silently advancing.
- Verify the actual deleted-file race on the mounted NTFS drive and force a real
  Btrfs queue overflow on an isolated fixture, including subsequent rename/delete
  operations and unchanged base-index publication.

### Faster broad searches

- Keep exact totals for supported one- and two-character filename searches on
  disk and read bounded candidates through the existing sort catalog. All nine
  sort orders returned 1,000 rows in 2–1,398 ms across five broad terms on the
  48.6M-record index. The installed GUI showed its first `a` results in 1.67 s.
- Keep eight recent query/sort combinations on disk, with exact counts and 4,096
  spare ordered rows. File events maintain both counters and pages in the catalog
  transaction; a bounded journal catches changes made during page preparation.
- Preserve matching semantics through safe fallback. Advanced options, scoped
  short searches, Unicode-sensitive `i`/`k`, unprepared longer terms and exhausted
  pages can still require compact search. This does not establish a universal
  three-second search limit.
- Existing-catalog preparation added 9.64 GB and took 285 seconds on this machine.
  Ordinary changes update the catalog incrementally without full rescans.
- Stop the watcher supervisor when a native reader marks the index stale, so a
  fanotify overflow is reported as failure rather than an apparently active service.

### Retained indexing access

- Retain Linux filesystem metadata access for each explicitly authorized account.
  Rebuilds use that grant, and installers restore it after replacing the scanner.
  Index settings can enable or repair access without terminal instructions.
- Run one continuous indexing service per authorized account, report readiness
  after native readers attach, and resume it after GUI rebuilds or cancellation.
  Closing during a rebuild schedules service recovery after the GUI exits.
- Remove whole directory prefixes through the durable delta instead of expanding
  every deleted descendant. Preserve recreated files through checkpoints,
  legacy-WAL upgrades, catalog recovery and compaction. Large moves remain bounded.
- Limit directory file-row allocation while keeping complete totals. Responsive
  tree columns reserve room for deep names and expose full paths on keyboard focus.
- Exclude Btrfs root backreferences from live records and remove existing bogus
  entries without stopping updates. Split metadata reconciliation by responsibility.
- Support local Trash and conflict-safe restore when the desktop service refuses
  an internal Btrfs subvolume. Preserve file contents and symlinks on failure.
- Invalidate searches after file actions and report completion only after queued
  operations finish. Batch overlay checks for matching search candidates.


### Index and search

- Add a disk-backed catalog covering both directions of Name, Path, Size and
  Modified sorting, with cursor paging through the complete result set.
  The prepared 70M-record index returns the first 1,000 ordinary rows in
  under 0.3 seconds on the development machine; cold preparation and typed
  searches have separate costs.
- Store the resolved live delta in a bounded SQLite cache instead of keeping
  every changed path in RAM. Preserve changes through checkpoints and restarts,
  recover older caches from the WAL, and never reuse a missing WAL's old rows.
- Index supported mounted filesystems from native metadata. Remove all directory
  walking and periodic full-reindex fallback paths. A failed or unsupported
  native lane reports an error instead of silently enumerating folders.
- Use closed-write events and Btrfs transaction deltas for ongoing updates.
  Bound continuous event bursts, fail closed on overflow, and isolate mount
  event descriptors from the writer's SQLite locks.
- Attach native readers and capture Btrfs cursors before index recovery. Report
  service readiness only after all accounts' readers and stores are ready;
  verify that saves queued during blocked recovery are preserved.
- Reuse decoded sibling blocks and release mapped data/descriptor windows after
  reads. Complete catalog rows are searchable without replaying the whole live
  delta for each ordinary listing.
- Use allocated disk bytes for sizes and map totals, including sparse files.
  Keep filename matching as the default; expose path matching, whole words,
  capitalisation, accents and regular expressions through the shared engine.

### Desktop

- Add custom window controls, a drive/folder scope picker including NVMe-only
  scope, and an icon/name category strip with vertical separators. All is the
  default filter at launch. The footer links to https://neutra.software.
- Run searches off the UI thread with cancellation and typing debounce. Preserve
  the last results while a replacement arrives and report invalid expressions.
- Restore display frame pacing instead of allowing uncapped Glow rendering.
- Add KDE Ctrl+K quick search with singleton focus handoff, plus an application
  menu entry for other desktops. Install the live-watch supervisor and retire
  the old scheduled-reindex unit and refresh script.
- Share Explorer menus between results, indexed-space rows, folder maps and
  graphs. Support rename, Open With, clipboard transfer, trash and Ctrl+Z restore;
  refuse overwriting an existing destination. External drag-out and multiple
  selection remain unfinished.
- Add bounded largest/smallest file-format maps and a hierarchy ball view.
  Show their sample limits explicitly. Keep dialogs resizable, constrain long
  breadcrumbs, and display future modification dates as dates rather than
  “Just now”.

### Verification and remaining work

- Repair this machine's live delta from its synchronized catalog and test
  repeated checkpoints, stale caches, generation changes and process isolation.
- Linux workspace tests, focused regression suites, Clippy with warnings denied,
  installer checks and owned virtual-desktop checks pass. Measurements, QA
  captures and incomplete UI delivery gates are recorded in
  `docs/feedback/production-readiness-validation-2026-10-09.md`.
- The 100 MB ceiling, full Explorer parity, NTFS/FUSE rename/delete journals,
  first-ingest watcher generation binding and complete accessibility remain
  unfinished. See `current-Debt.md`; this update is not a production sign-off.

## [0.1.24] - 2026-08-14

### Reliability

- Make compact-index and directory-summary path ordering explicit total byte comparisons for multi-million-record publications.

## [0.1.23] - 2026-08-14

### Reliability

- Build multi-million-record indexes on a dedicated 16 MiB stack so Windows publication does not overflow the CLI main stack.

## [0.1.22] - 2026-08-14

### Reliability

- Use stable heap-backed ordering for the multi-million-record directory-summary sidecar build.

## [0.1.21] - 2026-08-14

### Reliability

- Use a heap-backed stable compact-order sort so multi-million-record Windows indexes do not overflow the process stack during publication.

## [0.1.20] - 2026-08-14

### Reliability

- Preserve NTFS records whose parent is the pre-seeded MFT root even when the root sequence is not present in the parsed entry map.

## [0.1.19] - 2026-08-14

### Reliability

- Keep the Windows service scan path synchronous on the authenticated pipe thread while retaining asynchronous native scans on Unix.

## [0.1.18] - 2026-08-14

### Reliability

- Run Windows service scans on the authenticated pipe thread so native scan frames cannot block in a secondary thread before reaching the GUI.

## [0.1.17] - 2026-08-14

### Diagnostics

- Trace the native scan-begin emission boundary without logging indexed paths or metadata.

## [0.1.16] - 2026-08-14

### Diagnostics

- Record privacy-safe native worker lifecycle counts to distinguish scan-worker stalls from Windows service pipe delivery failures.

## [0.1.15] - 2026-08-14

### Diagnostics

- Record privacy-safe scan-preparation counts in helper logs to distinguish service discovery stalls from native worker or pipe failures.

## [0.1.14] - 2026-08-14

### Reliability

- Bound all Windows logical-drive discovery calls, including drive-type probing, so unavailable volumes cannot stall native scan preparation.

## [0.1.13] - 2026-08-13

### Reliability

- Bound Windows volume-filesystem metadata discovery so an unavailable local drive cannot stall scan preparation before native indexing begins.

## [0.1.12] - 2026-08-13

### Reliability

- Trace Windows scanner handshake, command dispatch, and response boundaries to diagnose pipe stalls without logging indexed paths or metadata.

## [0.1.11] - 2026-08-13

### Reliability

- Add bounded Windows scanner-service pipe lifecycle diagnostics to identify handshake and client-authentication stalls without exposing file metadata.

## [0.1.10] - 2026-08-13

### Reliability

- Remove redundant GUI-side Windows service process inspection so the authenticated pipe handshake cannot deadlock before a scan request.

## [0.1.9] - 2026-08-13

### Reliability

- Bound and cancel Windows client executable authentication so a blocked process-image lookup cannot stall the scanner service handshake or prevent later clients from connecting.

## [0.1.8] - 2026-08-13

### Reliability

- Authenticate Windows scanner clients after the protocol greeting but before accepting scan or query commands, preventing service-side pipe handshake deadlocks.

## [0.1.7] - 2026-08-13

### Reliability

- Complete the Windows scanner handshake before authenticating the helper process, preventing a pre-hello pipe deadlock during indexing.

## [0.1.6] - 2026-08-13

### Reliability

- Surface Windows scanner-service preparation errors instead of leaving the indexing client blocked indefinitely.
- Close one-shot scanner sessions after unrecoverable preparation errors while keeping the background service available for the next request.

## [0.1.5] - 2026-08-04

### Interface

- Let `neutrasearch index` scan the full machine without requiring a mount or output path.
- Make search, serve, MCP, GUI, and Pi reuse the last successful index location automatically; a missing search index triggers a full native-metadata build.
- Keep explicit index paths as optional overrides and reject directory-depth options.
- Persist generation-bound logical folder totals and direct children in a compact `.dirs` sidecar so Treemap preparation avoids materializing the full search index.

### Reliability

- Fix Windows upgrades when the existing scanner service binary path contains spaces.
- Stop the existing scanner through a non-blocking service-control request with a bounded status wait.
- Exercise clean install and in-place reinstall service paths before publishing Windows releases.

## [0.1.4] - 2026-07-23

### Interface

- Automatically repair a missing or empty startup index by scanning configured system roots instead of showing an idle zero-result screen.
- Show the complete indexed file list while the search box is empty; typed searches remain bounded for responsiveness.
- Distinguish active indexing and a genuinely empty index from hierarchy preparation in Treemap view.

### Repository

- Publish the cleaned source tree with concise project documentation and CI aligned to the intentionally tracked files.

## [0.1.3] - 2026-07-22

### Reliability

- Treat a missing Windows scanner service as a clean install instead of aborting setup, and retain a local service-install transcript for actionable failures.
- Exercise the compiled Windows installer and prove that `NeutrasearchHelper` reaches the Running state in CI before publishing it.

### Interface

- Start a whole-system native index automatically on first launch, selecting every fixed or removable local Windows drive and `/` on Linux and macOS; locations remain editable later in Settings.

## [0.1.2] - 2026-07-22

### Security

- Install the Windows raw-NTFS scanner as a LocalSystem service behind a local-only named pipe; both endpoints authenticate the opposite process before exchanging selected roots or records.
- Force service builds into a non-reparse Program Files directory with deterministic SYSTEM/Administrators-write and Users-read/execute ACLs.
- Keep approved-root validation and filtering inside the privileged helper; arbitrary local processes cannot submit framed scanner commands through the service pipe.

### Reliability

- Fix case-insensitive Windows scope checks when index records and selected roots use different slash styles.
- Emit `/` rather than an empty path for the Btrfs root inode, allowing whole-root compact index builds to publish successfully.
- Register, start, upgrade, recover, and uninstall the Windows scanner service with the administrator-approved setup, eliminating per-scan UAC prompts for installed builds.
- Bump the helper compatibility build to 9 and add persistent Windows service logs under `%ProgramData%\Neutrasearch`.

### Performance and interface

- Show the newest indexed entries when search is empty, with deterministic path tie-breaking for equal sort values.
- Replace per-record ancestor updates in the disk hierarchy with direct-folder collection and bottom-up aggregation, and prepare that model alongside compact-index publication instead of showing a second blocking phase.

## [0.1.1] - 2026-07-22

### Distribution

- Include Windows, Linux, and macOS installers in the downloadable release `SHA256SUMS` manifest.

## [0.1.0] - 2026-07-22

### Security

- Resolve privileged scan requests against trusted operating-system mount metadata.
- Reject environment-selected helpers during `pkexec` elevation.
- Add owner-only, no-follow WAL/lock handling and exclusive temporary base creation.
- Bound helper queries, scan requests, delta batches, and protocol frames.
- Make MCP fail closed without an explicit index and apply allowed-root filtering before ranking and result limits.
- Store selected-folder settings in an owner-only configuration directory and file on Unix.
- Carry approved folder roots through protocol v7 and filter inside the privileged helper before any records cross back into the user process.

### Reliability

- Add checksummed compact index format v3.
- Persist stale watcher state and require a full rebuild to clear it.
- Fail closed on complete WAL frames with invalid checksums.
- Add recoverable automatic base/WAL compaction and persistent-reader generation/stale-state handling.
- Serialize full rebuilds against live delta writers and exclude `/.snapshots`, `/proc`, and `/sys` by default.
- Bump the helper protocol to v7 and helper build compatibility level to 8; every scan requires approved roots, ends with an explicit completion frame, and empty mount/root lists scan nothing rather than silently selecting every volume.
- Stage rebuild records and publish reachable selected locations together; unavailable lanes no longer block successful locations, while a total scan failure preserves the last complete index.
- Retry offline mounted servers without treating them as local permission failures; keep authentication, integrity, and unsupported-platform errors visible.
- Bring native initial-index CLI and volume discovery flows to Linux, Windows, and macOS; classify real Windows NTFS volumes and discover user-visible macOS APFS/HFS volumes.
- Make Windows/UNC scopes and Treemap roots portable, and fall back to macOS bulk metadata traversal without parsing localized Spotlight status text.

### Interface

- Replace the green accent with a subdued slate/periwinkle desktop palette and use the real Neutrasearch logo in the app, Wayland window, Windows executable, Linux shortcut, and macOS launcher bundle.
- Reduce the first-run screen to a persisted multi-folder picker and one Scan action; keep setup active until the first usable index exists and retain a compact, actionable retry state after authorization failures.
- Reduce the menu bar to three task menus plus Help, add Ko-fi and Patreon links, make diagnostics selectable, and add a direct copy shortcut for selected paths.
- Let search and results dominate the default workspace: move expert match/scope/case/regex controls into Search, collapse view choices into a dropdown, remove duplicate status chrome, and show only active non-default search modifiers.
- Add conflict-free result shortcuts (`Ctrl+Up/Down`, `Ctrl+Insert`), focused onboarding actions, and a dedicated no-locations recovery state.
- Group locations, index status, maintenance, scanner details, and network controls with progressive disclosure; adding or removing a location now refreshes the index automatically.
- Make every details-table header directly sortable, expose selected-result actions, and distinguish invalid regular expressions from valid searches with no matches.
- Publish an Inno Setup Windows x64 installer, Debian x64/ARM64 packages, and macOS Intel/Apple Silicon disk images alongside portable release archives.
- Switch the Linux desktop renderer to low-latency Glow without vsync and bound event draining during resize frames.
- Add explicit Linux administrator rebuild actions backed by `pkexec` and trusted root-owned helper validation.

### Distribution

- Add deterministic portable archive tooling, release checksums, and cross-platform release automation.
- Add the `pi-neutrasearch` Pi package with a workspace-confined, read-only, token-efficient indexed path tool; it defaults to 20 relative paths, paths-only JSON transport, and a 6,000-character output cap.
- Make `pi install npm:pi-neutrasearch` install the matching native application through OS/CPU-constrained optional packages, with `/neutrasearch-setup` for explicit first-index approval and no postinstall scan or privilege action.
- Add query-client `--scope` and `--json-paths` options for trusted agent integrations without unnecessary metadata payloads.
