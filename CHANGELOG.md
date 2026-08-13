# Changelog

All notable changes are documented here. Neutrasearch follows semantic versioning; pre-1.0 releases may contain intentional compatibility breaks described in the release notes.

## [Unreleased]

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
