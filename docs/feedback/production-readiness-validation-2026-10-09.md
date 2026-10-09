# Production readiness progress

The owner's requirements are preserved verbatim in
`production-readiness-request-2026-10-08.md`. Version 0.1.25 is installed at
the same `/usr/local/bin/neutrasearch` path used by the Start-menu entry.
The user-local commands now point to that installation. The live service is
enabled; its obsolete index timer and refresh script have been removed.

This is a development update, not a production-readiness declaration.

## Measured behavior

Measurements used the existing 70,317,612-record index. No drive was walked
or rebuilt for these measurements. GUI checks ran on owned Xvfb/Openbox
displays, never the real desktop.

| Check | Result |
|---|---|
| GUI launch to first 1,000 visible rows | 0.738 seconds upper bound, including screenshot/OCR overhead |
| Name ascending / descending, first 1,000 | 0.245 / 0.256 seconds |
| Path ascending / descending | 0.004 / 0.016 seconds |
| Size ascending / descending | 0.048 / 0.283 seconds |
| Modified ascending / descending | 0.015 / 0.003 seconds |
| Three closed writes to live catalog, immediately after service ready | 0.178 / 0.230 / 0.203 seconds |
| Save while recovery was blocked behind a SQLite writer | Readiness withheld; queued save published 0.315 seconds after releasing the lock |
| KDE quick-search registration | Ctrl+K assigned to the installed desktop entry; component active |
| Native reader lock regression | Reader processed real events while the parent retained its POSIX lock |

These are warm measurements on this machine, with a prepared disk catalog.
They do not establish a three-second bound for typed searches, cold catalog
preparation, large folder operations or every supported platform. An earlier
probe started before watcher attachment missed its first six-second window.
Readers and Btrfs cursors now attach before durable recovery, the helper
acknowledges readiness, and systemd waits for every account's acknowledgement.
`scripts/check-watch-startup.py` blocks catalog recovery, saves a real file,
checks that readiness is withheld and verifies that the queued save arrives.
This closes the measured recovery window; first-ingest generation binding and
newly mounted drive detection remain separate work.

The 100 MB ceiling is **not established and remains unmet in summed RSS**.
The latest ten-second sample measured 94,755 KiB combined PSS and 181,936 KiB
summed RSS across the GUI, both supervisors, durable helper and five readers.
The GUI alone measured 123,508 KiB RSS and 67,518 KiB PSS. PSS accounts for
shared pages proportionally; RSS counts those pages in each process. The
service also charges reclaimable filesystem-cache pages to its cgroup.
An earlier sample measured 105,803 KiB PSS and 188,044 KiB summed RSS, exceeding
the ceiling even in PSS. Restored display frame pacing preceded the latest
sample: the software-rendered GUI used about 20% of one core, while the busy
Btrfs helper used about 59%. Earlier values were about 80% and 64%; these are
different busy-write windows, not a controlled performance comparison or idle
CPU measurements. Peak, cold-ingest and hardware-rendered resource limits
remain unverified.

## Durability repairs

- Mount-event collection now runs in separate helper processes. Closing a
  fanotify event descriptor must not release SQLite locks held by the durable
  writer. A real crash dump located the fault inside the catalog's shared
  memory mapping; the isolated-reader regression preserved the parent's lock.
- Readers attach and sweep cursors are captured before opening the durable
  store. Missing Btrfs metadata roots fail startup rather than silently omitting
  reconciliation; a targeted watch only sweeps its selected mounts.
- Checkpoints retain resolved upserts and removals while publishing the new
  cursor. Repeated checkpoints previously cleared those rows. Cache format 2
  invalidates the older cache representation, and missing WALs cannot reuse
  rows from a prior generation. The next checkpoint size persists across a
  restart instead of immediately reprocessing the large existing WAL.
- This installation's affected delta was recovered from its synchronized
  catalog: 1,080,621 removed/replaced base addresses and 236,498 live records.
  Recovery holds the writer lock, rejects an unsynchronized catalog or a
  changed original base, and writes a separate output before publication.
- Directory child lookups reuse one decoded block. Bulk reads release both
  data and descriptor fault-around windows; a narrow eviction had allowed
  clean mapped pages to accumulate in RSS.

## Verification

The workspace suite passed. Subsequent focused runs passed 79 core tests,
30 GUI tests, 32 helper tests, and the ZFS tests. The streaming parity test
also passed. Clippy passed for all workspace targets with warnings denied;
formatting, diff whitespace, packaging, no-walk and build-hygiene checks
passed. An independent read-only review found no established blockers in the
final checkpoint, cache, recovery and reader-process changes. Subsequent
startup changes passed the focused core/helper/GUI/watch suites, workspace
Clippy, formatting, shell syntax, systemd unit verification, the no-walk guard
and the native blocked-recovery probe. Release binaries were rebuilt and the
five installed/package/release hashes match. Startup review identified a false
ready state when no supported mounts existed; startup now refuses that state.

The native package contains matching binaries and the live-watch/shortcut
tooling. Actual Start-menu path resolution and installed binary versions were
checked. Running MCP sessions were not terminated during installation.

## Recorded GUI interactions

The disposable QA fixture contains 1,354 real files. Its screenshots are
under `qa-2026-10-09/`; full-index captures containing private filenames stay
outside the repository.

- Custom maximize and restore changed between 1,180×760 and 1,440×960.
- Minimize produced an iconic window state; restoring and resizing to
  800×570 held the requested dimensions.
- Ctrl+K focused the field; `QA Documents 000` returned exactly one file.
- Folder chevrons expanded the hierarchy and navigation changed the map root.
- Folder, file-format and ball views opened; the map's right-click menu exposed
  the shared Explorer actions.
- Quick search opened on the virtual display; repeated launch focused the
  existing instance, and Escape closed it.
- Earlier fixture checks trashed and restored an actual file with Ctrl+Z;
  the focused transport suite covers overwrite refusal and queued undo.

Global Ctrl+K was not invoked on the real desktop. External application launch,
every menu permutation, drag-out and full keyboard/screen-reader use have not
been exhaustively verified.

## Antislop delivery gate

- Hard gate: **FAIL / incomplete**. R-35 lacks exhaustive interaction coverage;
  full accessibility remains outstanding while AccessKit is disabled.
- Purpose gate: **PASS**. Palette, fonts, density, category treatment and
  elevation derive from `DESIGN.md` and the owner's explicit requests.
- Liveliness: **PASS**. Declared 1/1/1 dials, the search field as focal point,
  restrained red accent and repeated pro-audio typography are visible in QA.
- Craftsmanship: **FAIL / incomplete**. External drag-out, multiple selection
  and full Explorer parity are unfinished; the memory target is unproven as
  a ceiling and unmet in the recorded sample.

The UI is not signed off as finished. `current-Debt.md` records the remaining
functional and performance work.

## Ponytail audit

`delete:` Retired ZFS walk opt-in parser/assignment, probe fields and stale fallback
documentation. Replacement: nothing; native indexing remains mandatory.
Paths: `crates/neutra-helper/src/main.rs`, `crates/neutra-zfs/src/lib.rs`.
Removed in the subsequent authorized code cleanup.

net: -24 lines, -0 deps.
