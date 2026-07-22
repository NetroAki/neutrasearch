# Changelog

All notable changes are documented here. Neutrasearch follows semantic versioning; pre-1.0 releases may contain intentional compatibility breaks described in the release notes.

## [Unreleased]

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
- Publish an Inno Setup Windows x64 installer alongside the portable release archive.
- Switch the Linux desktop renderer to low-latency Glow without vsync and bound event draining during resize frames.
- Add explicit Linux administrator rebuild actions backed by `pkexec` and trusted root-owned helper validation.

### Distribution

- Add deterministic portable archive tooling, release checksums, and cross-platform release automation.
- Add the `pi-neutrasearch` Pi package with a workspace-confined, read-only, token-efficient indexed path tool; it defaults to 20 relative paths, paths-only JSON transport, and a 6,000-character output cap.
- Make `pi install npm:pi-neutrasearch` install the matching native application through OS/CPU-constrained optional packages, with `/neutrasearch-setup` for explicit first-index approval and no postinstall scan or privilege action.
- Add query-client `--scope` and `--json-paths` options for trusted agent integrations without unnecessary metadata payloads.

## [0.1.0] - Unreleased

Initial pre-1.0 release line.
