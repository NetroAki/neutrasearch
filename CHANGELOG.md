# Changelog

All notable changes are documented here. Neutrasearch follows semantic versioning; pre-1.0 releases may contain intentional compatibility breaks described in the release notes.

 ## [Unreleased]


### Added

- One-time elevation on single-user machines: `packaging/linux/49-neutrasearch.rules`
  approves the indexing helper for the local active session, so scans never
  prompt for a password again after it is installed.
- The live watch no longer crash-loops on btrfs-subvolume layouts: `neutrasearch-watch-all`
  skips subvolume mounts (filesystem fanotify marks return EXDEV there since kernel 6.8)
  and exits quietly when nothing is markable, and the unit only restarts on failure.
  The weekly reindex plus the GUI rescan on launch stay the safety net there.

- Wire the directory-summary overlay end to end: the helper protocol gained a `DirectorySummary` request served from the live base + delta pair (protocol version 8), the MCP server gained a `neutra_directory` tool, and the GUI treemap projects live totals from the overlay whenever a watch helper has pending WAL changes instead of showing the last-published sidecar.

### Internal

- Split the mixed-concern modules flagged by the code-quality pre-scan (complete): every source file in the workspace is now under 1,000 lines and every function under 120: `neutra-query` (argument parsing vs NDJSON service), `neutra-mcp` (server vs index store vs scope policy vs tool surface), `neutra-ntfs` (orchestration vs byte-level geometry vs record interpretation), `neutra-helper` (entry points vs protocol loop vs scan orchestration vs durable store), `neutra-gui` (app state vs out-of-process transport), and `neutra-macos` (Spotlight selection vs getattrlistbulk fallback). No behavior change.
- Ship a Linux polkit policy (`packaging/linux/com.neutrasearch.helper.policy`, `auth_admin_keep`) so elevated scans prompt for the password once per five minutes instead of on every scan, with packaging instructions.

### Search

- The shell follows the second mock: text filter tabs, amber access banner
  with Review/elevated actions, tabbed sidebar (Locations, Index, Network,
  Maintenance) with per-location status dots, real progress percentages
  from previous per-mount totals, and a status bar with index state.
- The Locations-and-index dialog is a docked sidebar with tabs (Search
  Locations, Index Status, Scanner Details, Index Maintenance, Network
  Folders); the filter row uses icon pills and the toolbar shows search
  time, list/grid shortcuts, and per-row menus.
- The file-type filter always resets to All on launch, and is no longer
  persisted: a leftover Audio or Images preset silently hid most results.
- The first-index view names every drive with its live state (indexing path
  or finished object count), so scan progress is visible per drive.
- Search matches file/folder names by default; folder paths only match when
  asked (Search > Match: file names only / names + folder paths / paths only).
- A filter row under the search box offers All, Audio, Images, Video,
  Programs, Compressed, Documents, and Folders presets (extension groups
  shared by every client; Programs also matches `+x` binaries).
- New match options: whole words only, and ignore accents (café = cafe).
  Capitalisation matching keeps its plain-language label.
  Capitalisation matching keeps its plain-language label.
- Move regex, case sensitivity, and name/path matching scope into the query engine so every client (GUI, CLI, MCP) matches identically; the GUI no longer re-filters engine results, which previously broke the `ext:`/`kind:`/`size:`/`under:` syntax and silently discarded engine matches.
- Highlight the matched part of file names in Details and List views using the same compiled matcher as the engine.
- Clicking an active sort header flips its direction (name, path, modified, size); the engine gained the matching descending/ascending sort keys.
- Default the GUI to relevance ranking while searching (name-prefix and exact-name matches rank first) instead of interleaving matches by modification time.
- Show the full matched count when results are capped ("1,000 of 40,312 results") instead of an unqualified count.
- Report invalid regular expressions in the empty state instead of silently showing stale results.

### ZFS

- The opt-in ZFS enumeration is now first-class: `neutrasearch index --zfs-enumerate` requests it as an authenticated scan parameter (protocol v9), so it works through pkexec elevation where environment variables are stripped. `neutrasearch-helper --zfs-probe` reports which ZFS lanes a machine supports.
- The ZFS lane now works: an explicit, documented single-pass enumeration (`NEUTRASEARCH_ZFS_ALLOW_WALK=1`) indexes a dataset with one sequential openat/getdents64/fstatat sweep that never follows symlinks and never crosses filesystem boundaries. Without the flag the lane still refuses, with the remedy in the message. The tested `zfs diff` parser and snapshot command builders remain the update story, and the `zfs-libzpool` feature remains the intended native lane.

### Fixed

- Scan progress names every drive with its live state, and scans can be
  cancelled mid-run (Unix): staged batches are discarded and the previous
  index stays searchable.
- Startup no longer burns minutes and gigabytes before showing a window:
  the compact search splits candidates into a fixed set of groups that
  stream with pruned top-N lists (a 51M-record empty query runs ~2 s wall),
  and fully-free pages are handed back after big searches. Residential
  anonymous memory on that index went ~15 GB to ~300 MB, the rest being
  reclaimable mapped file pages.
- Diagnostics rows truncate earlier so long lane statuses cannot force the
  dialog wider than its default size; the About window is resizable.
- The NTFS lane never walks: an unreadable raw volume hard-fails instead of
  grinding the mounted tree (slow, hammers the drive). Unreadable means the
  setup is wrong (elevation, versions), never the drive.
 - Fetch tree folders on demand from the base with pages released as the
   scan advances: browsing holds tens of megabytes under a 48 MB cache
   instead of materializing tens of gigabytes (a 14M-file listing completes
   under a 70 MB cgroup cap). In-RAM index builders are now test-only;
   production compaction and single-mount builds stream through spills.
   Removed the btrfs serial scan path and its debug flags, GUI reference
   mode, legacy env-var twins, NTFS/btrfs progress prints, and the orphaned
   WAL-frame and sidecar-stream readers left behind by the fetch rewrite.

 - Stream the tree model straight from sidecar frames and move entry strings
   instead of cloning them: opening the disk map no longer materializes the
   full entry set or duplicates every path, cutting tree-open memory by an
   order of magnitude on large indexes. Record fallbacks feed the same
   shared aggregation block by block instead of decoding the whole base.

- Uppercase non-ASCII search terms ("CAFÉ") never matched lowercase file names: term needles were not case-folded before the Unicode comparison path. Needles are now folded once per query and the haystack comparison is allocation-free.
 - Follow the NTFS $MFT runlist continuation through $ATTRIBUTE_LIST extension records: heavily fragmented volumes (80+ runs overflowing the base record) previously failed closed partway through the $MFT, leaving ~7% of records unindexed. Nameless list entries are decoded at their real 32-byte stride, continuations are matched by attribute id and sequence, and extension runs are rebased and contiguity-checked before use.
 - Repair the Btrfs parallel-scan arity breakage (callee gained a tree-id parameter, caller not updated) so the workspace builds again; the default path searches the tree of the opened mount exactly like the serial path.

 - The GUI no longer asks which folders to scan: it indexes everything by default and re-scans on every launch, keeping the previous results searchable until the new scan lands. Locations stay adjustable from the index settings, and NEUTRASEARCH_NO_AUTOSCAN=1 opts out of the launch scan.
 - The tree view builds its model without per-file string expansion (names and extensions derive when rows paint), holds folders in a hash map, virtualizes rows so only visible ones paint, and ranks treemap tiles with a top-256 partial selection instead of sorting every child.
- Ship a Linux background refresh (`neutrasearch-index.timer`, 15 min after boot then daily): a root oneshot service re-indexes every interactive user machine index in place, so results stay fresh while the GUI is closed. Failed or empty builds keep the previous index.
 - Stream full-machine builds through disk spill (sorted runs, external postings sort) instead of staging every record in RAM: 90M-record hosts previously peaked past 25 GiB and tripped the OOM killer. Output layout is unchanged and covered by a parity test; scans and the GUI adopt the published base without a resident copy.

 - Track on-disk bytes alongside apparent size: every lane records physical
   usage (btrfs extent bytes, ext4 block counts, NTFS allocated size,
   `st_blocks` for walks), folder totals and treemap tiles use it, and sparse
   files such as multi-terabyte Docker raw images no longer report their
   apparent size as disk usage. File search results still show apparent size.
 - Version every on-disk and on-wire record layout explicitly instead of
   probing bytes: the compact base moves to v4 (v3 stays readable with disk
   falling back to size), the directory sidecar to v2 (v1 stays readable),
   the delta WAL to a new magic (old logs replay then migrate on writer
   open), legacy snapshots gain an envelope, and the helper protocol moves
   to v10 so mixed-version peers fail the handshake instead of shifting
   fields. Upgrades keep existing indexes; new scans write new layouts.
 - Repair the streamed sidecar writer hashing the header into the checksum
   readers exclude, which made every streamed sidecar fail verification and
   forced the tree onto the slow full-record fallback. Raise the sidecar
   size caps to host scale (2 GiB compressed, 32 GiB streamed payload) so a
   100M-record sidecar opens instead of being rejected.

### Performance

- Parallelize compact-index block encoding and trigram posting during builds, and parallelize compact search block decoding; compaction holds the store write lock for a much shorter time.
- Open indexes without hashing the whole payload on read paths (GUI startup, MCP, CLI, persistent query); full checksum verification stays on write paths and `CompactIndex::open`.
- Resolve delta paths in the directory-summary overlay by binary search instead of a full index scan, making overlay construction proportional to the change set rather than the whole index.
- Virtualize the List and Grid result views and cap the home view at 10,000 entries, removing per-frame widget construction for every hit on large indexes.
- Coalesce watched filesystem events with a 250 ms quiet window and one WAL flush per batch instead of a flush and fsync per event.
- Remove per-record path normalization and `PathBuf` joins from the scan hot loop; root and exclusion prefixes are computed once per scan.
- Compare names case-insensitively without per-comparison allocations in sort paths.
- Size Btrfs scan preallocations and shard counts to the machine instead of reserving for 13 million records and 17 fixed threads.
- Precompute canonical path keys for directory-summary overlays and share one sort comparator and path-safety predicate across engines instead of per-module copies.

### Reliability

- Treat directory renames, unpaired moves, and watch-queue overflows as degraded (log once, keep serving) instead of disabling every search until a full reindex; hard watcher failures still fail closed.
- Stop routing non-scanner errors (failed file open, settings write) to the "retry as administrator" banner; only scanner and index failures do.
- Persist GUI view, filter, sort, and search-option choices across launches.

### UI

- Enter or Down now leaves the search box into the results and opens the selection (Everything-style); Escape clears the search; keyboard selections scroll into view.
- Remove the inert "Run in background" banner button and decorative row dots.
- Document the search filter syntax in `neutrasearch help` and `neutrasearch search --help`.
- `neutrasearch search --no-build` fails with an actionable message instead of silently starting a full privileged index build.
- A missing helper binary now suggests installing it beside the executable or setting `NEUTRASEARCH_HELPER`.
- Diagnostics shows when the index was last updated (from the index file's publish time).
- Treemap views say "Indexed space" instead of "Local disk" so indexed network shares are not mislabeled.
- The Locations & Index panel stays hidden until asked for (File > Locations and
  index, the Index details button, or the access-banner Review link); the old
  modal dialog survives only for first-run setup.
- The GUI now looks like the Neutraudio plugins and shell (`DESIGN.md`, from the
  Neutraudio §36 spec, its shell tokens, and its mockup): a slate-950 canvas
  with slate-900 panels, red as the single active accent, Inter and Roboto Mono,
  pill filter chips with a red active state, tracked uppercase table headers and
  panel titles, a red selection bar, recessed wells for the location list, a
  bordered search field with the magnifier inside, 10px scrollbars, and a 2px
  red focus ring. File-type badges are outlined by kind (audio violet, images
  and video green, folders and archives amber, PDF red).
- Text that used the low-contrast muted grey now uses surface-400, accent text
  uses a lighter red so it clears 4.5:1 on every surface, and violet appears only
  as outlines and dots.
- Result paths show the tail with a leading ellipsis and fit their column;
  inactive sort headers no longer show arrows.
- The results canvas moved to the reference blue-slate tone and the kind tabs
  gained spacing and size to match the UI mock.

### Size

- Ship embedded fonts zstd-compressed (about 21.5 MB to 12 MB), roughly halving the GUI binary; decompression is a one-time startup cost.
- Strip release binaries.

### Removed

- Dead treemap `file:` index-selection parsing and the decorative details-row hover dots.

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
