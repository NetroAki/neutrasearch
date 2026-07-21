# Changelog

All notable changes are documented here. Neutrasearch follows semantic versioning; pre-1.0 releases may contain intentional compatibility breaks described in the release notes.

## [Unreleased]

### Security

- Resolve privileged scan requests against trusted operating-system mount metadata.
- Reject environment-selected helpers during `pkexec` elevation.
- Add owner-only, no-follow WAL/lock handling and exclusive temporary base creation.
- Bound helper queries, scan requests, delta batches, and protocol frames.
- Make MCP fail closed without an explicit index and apply allowed-root filtering before ranking and result limits.

### Reliability

- Add checksummed compact index format v3.
- Persist stale watcher state and require a full rebuild to clear it.
- Fail closed on complete WAL frames with invalid checksums.
- Add recoverable automatic base/WAL compaction and persistent-reader generation/stale-state handling.
- Serialize full rebuilds against live delta writers and exclude `/.snapshots`, `/proc`, and `/sys` by default.
- Bump the helper protocol to v5 and helper build compatibility level to 6; every scan now ends with an explicit completion frame.
- Stage rebuild records and publish a replacement only when every discovered native lane succeeds, preserving the last complete index on partial failure.
- Bring native initial-index CLI and volume discovery flows to Linux, Windows, and macOS; classify real Windows NTFS volumes and discover user-visible macOS APFS/HFS volumes.
- Make Windows/UNC scopes and Treemap roots portable, and fall back to macOS bulk metadata traversal without parsing localized Spotlight status text.

### Distribution

- Add deterministic portable archive tooling, release checksums, and cross-platform release automation.
- Add the `pi-neutrasearch` Pi package with a workspace-confined, read-only, token-efficient indexed path tool; it defaults to 20 relative paths, paths-only JSON transport, and a 6,000-character output cap.
- Make `pi install npm:pi-neutrasearch` install the matching native application through OS/CPU-constrained optional packages, with `/neutrasearch-setup` for explicit first-index approval and no postinstall scan or privilege action.
- Add query-client `--scope` and `--json-paths` options for trusted agent integrations without unnecessary metadata payloads.

## [0.1.0] - Unreleased

Initial pre-1.0 release line.
