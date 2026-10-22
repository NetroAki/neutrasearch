<!-- doc-topic: release-history -->
# Changelog

All notable changes are documented here. Neutrasearch follows semantic versioning; pre-1.0 releases may contain intentional compatibility breaks described in the release notes.

## [Unreleased]

- Windows maps the index file instead of reading it into memory. Replacing an
  index that a reader has mapped renames the old file aside.
- Linux cross-volume moves hold each file aside before deleting it, so a file
  saved over the original at that moment survives. Folders with hard links no
  longer report a false failure.
- Copy errors show the first 2 KB of the tool's message, and move errors no
  longer rewrite file names.
- Partial index files left by a killed writer are removed on the next publish.
- Publishing a folder tree ignores abandoned temporary copies so interrupted
  publication cannot block later index updates.
- Publish rebuilt browse catalogs through SQLite so an existing reader's
  write-ahead log cannot corrupt the replacement catalog.
- Defer compaction until startup metadata replay has saved its checkpoint,
  keeping live updates writable and the replay's base generation stable.
- Give queued live updates a turn between recovery batches, and prevent an
  older recovery plan from overwriting newer saves or removals.
- Release workflows pin every action to a commit and Rust to 1.99.1.

## [0.1.31] - 2026-10-21

- Restrict the Windows scanner pipe to the installer-approved account, SYSTEM
  and administrators; keep that approval across upgrades. The GUI cannot create
  competing pipe instances and verifies the installed service before talking.
- Stop the Linux helper if memory protection fails.
- Link the Windows C runtime so clean installs start.
- Keep the last complete index when a scan lane fails; stage rebuilds and
  recover interrupted publication without discarding live edits.
- Capture Btrfs recovery cursors first and replay later changes before
  announcing live updates. Index every Btrfs hardlink alias.
- Use private spill storage, reject lossy filenames, enforce MCP roots, and
  include licence notices in npm packages.
- Preserve Unix backslashes and macOS case in folder listings; earlier Unix
  folder summaries need an index rebuild.
- Reject corrupt allocated NTFS records and bound attribute-list sizes and
  \$MFT runs by the device size, so forged volumes fail with an error.
- Write Windows service and installer logs in the restricted installation
  folder. Windows and macOS copies publish atomically and keep
  file streams, forks and attributes; folders with streams and cross-volume
  moves are refused. Linux cross-volume moves stage first. Live updates resume
  after a crashed rebuild. Pi setup no longer
  replaces files or app bundles it did not create.
- Report offline coverage limits for ext4, ZFS and NTFS snapshots.
- Debian package: install on Debian 13 and current Ubuntu, never stall an
  upgrade on a watcher, and remove permission files on purge.

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
