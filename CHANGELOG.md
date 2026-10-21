<!-- doc-topic: release-history -->
# Changelog

All notable changes are documented here. Neutrasearch follows semantic versioning; pre-1.0 releases may contain intentional compatibility breaks described in the release notes.

## [Unreleased]

## [0.1.31] - 2026-10-21

- Restrict the Windows scanner pipe to the account approved during installation,
  SYSTEM and administrators. Preserve the approved account during upgrades and
  reject service startup when that approval is missing.
- Prevent the GUI from creating competing scanner pipe instances or granting
  a pipe server impersonation rights.
- Stop the Linux helper and watcher if process memory protection cannot be set.
- Link the Windows C runtime into the binaries so the GUI and scanner start on
  clean Windows installations without a separate Visual C++ redistributable.
- Verify the scanner pipe belongs to the running installed service before
  exchanging any protocol messages.

## [0.1.30] - 2026-10-21

- Stage Linux copies and moves before publishing them without replacing an
  existing destination, including on coreutils 9.4. Use atomic no-replace
  renames on Windows and macOS.
- Merge spill chunks using the same path comparison as their initial sort.
  Resolve the macOS `/var` alias, flush Windows staged files through writable
  handles and compare search-cache paths consistently. Fail macOS scans on
  metadata errors other than vanished files.
- Ship third-party licence notices. Gate releases on tests, lints, audit and
  dependency review; run dependency review on pushes. Move release tools into
  `packaging/tools`, pin Pi binaries to the release and make the passwordless
  polkit rule an opt-in example.
- Remove unused tracing, shaping and Illustrator dependencies; upgrade
  event-listener to 5.4.2. Document platform gaps, MCP scope and in-memory undo.
- Known limits at this release: the Windows service accepted every local account;
  Linux watch retains CAP_SYS_ADMIN and CAP_DAC_READ_SEARCH for fanotify. Only
  Linux supports live updates and trash. These limits remain documented in the
  platform table and privacy notes; 0.1.31 restricts Windows scanner access.

## Earlier releases

The complete, unchanged history through 0.1.30 is preserved in the
[0.1.30 changelog](https://github.com/NetroAki/neutrasearch/blob/v0.1.30/CHANGELOG.md).
Published downloads and notes are available in
[GitHub Releases](https://github.com/NetroAki/neutrasearch/releases).
