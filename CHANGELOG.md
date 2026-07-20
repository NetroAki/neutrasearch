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
- Bump the helper protocol to v4 and helper build compatibility level to 5.

### Distribution

- Add deterministic portable archive tooling, release checksums, and cross-platform release automation.
- Distribute the Pi package separately from the application repository.

## [0.1.0] - Unreleased

Initial pre-1.0 release line.
