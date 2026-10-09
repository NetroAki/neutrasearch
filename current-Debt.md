# Debt ledger

Deliberate, recorded shortcuts and gaps. Repay conditions are concrete; entries are removed when repaid. See also the CHANGELOG for what was already repaid.

## Benchmarks (this machine, 2026-09-19)

Two-device Btrfs RAID0 (nvme0n1p2 + nvme1n1p1, 3.6 TiB total): `@` tree 3.13M records in 34.2 s cold / 2.42 s warm (~1.29M records/s warm); `@home` tree 73.05M records in 491 s cold (~149k records/s). RAID0 needs no special handling: Btrfs metadata trees are filesystem-global, so TREE_SEARCH on any mount enumerates files striped across all member devices — coverage is per-subvolume, not per-device, and the mount selection already scans every mounted subvolume. Observation: the last shard (objectid ≥128M, 38.4M nodes) dominates `@home` scan time (485 s of 491 s wall) — worth revisiting shard balance if that tree matters.

## ZFS

- **`libzpool` ZAP backend remains a placeholder** (feature `zfs-libzpool`). Doing it properly is a project, not a patch:
  1. Pin an OpenZFS release (`OPENZFS_REV`, e.g. `zfs-2.3.x`) in-repo; everything derives from the pin.
  2. Generate FFI with bindgen against that tree's headers (`sys/dmu.h`, `sys/zap.h`, `sys/sa.h`, `sys/zfs_znode.h`, `sys/dnode.h`) with a strict symbol allowlist — never hand-written, because the internal signatures shifted across 2.1→2.2.
  3. Link against the pinned tree's libzpool: a userspace DMU that reads vdevs directly, no kernel module needed on the indexing host.
  4. Traversal: objset → master node (object 1) → ZPL root directory (object 3) → recursive ZAP walk (`zap_cursor`) mapping name→object; per object `dmu_bonus_hold` for `znode_phys` (times, generation, rdev) **plus SA attribute lookups for mode/uid/gid/size** — modern pools keep those in the SA table, and the SA framework is the part that demands generated bindings.
  5. Runtime ABI gate: probe (open objset, read master node) before emitting records; mismatch fails closed with `--zfs-probe`-style diagnostics.
  6. Test on a file-vdev pool (libzpool opens them userspace-style, like zdb) inside a privileged CI container with zfsutils; populate a known tree and assert records.
  Unblock: an OpenZFS checkout + a privileged CI job. Estimated as a focused project (the traversal is small; the pinned-tree CI and SA handling are the work).

## Oversized modules (from the repo pre-scan; do not grow further)

Still oversized — split at the next feature that touches them, don't split speculatively:
- `neutra-core/src/dir_summary.rs` (1,129 lines) and `compact.rs` (1,360 lines): on-disk format code is cohesive; split format decoding from search and lifecycle when those concerns next change.

## Accepted micro-costs

- **Folder summary is built at GUI launch, not at index publish.** `neutra-core/src/dir_tree.rs` derives the top-three-level summary (`<index>.tree`) by streaming the existing `.dirs` sidecar once per generation, so the first launch after an index update builds it in the background (about 20 seconds on 61M records) and the tree waits for it. Repay by emitting it from the sidecar writer so it exists the moment the index is published.
- **Folder summary ignores the live delta.** Totals for the top three levels come from the published base; changes in the live WAL appear in those totals after the next publish. Folders below depth three still merge the delta exactly through `list_directory`.
- **Folders below depth three stream their own subtree.** A very large fourth-level folder costs time proportional to its subtree (about a second for 10M records). Repay by raising `MAX_DEPTH` in `dir_tree.rs` or giving the summary a random-access layout.
- **No automated test for the tree panel's click and keyboard behaviour.** It was checked by clicking through the running app; the flattening and navigation logic in `ui/tree_panel.rs` has no unit test.
- **Phosphor icons are not bundled.** DESIGN.md (from the Neutraudio spec) names Phosphor Icons as SVG assets; the GUI still draws its small glyphs (home, drive, database, view toggles, search) as vector shapes in `ui/icons.rs` and `ui/widgets.rs`. Taken 2026-10-03 to keep the binary and dependency set unchanged during the token rework. Repay by bundling the needed Phosphor SVGs and rendering them through egui's SVG image loader.
- Accent folding covers Latin diacritics only (`strip_accent` in `neutra-core/src/matcher.rs`); other scripts still compare exactly. Repay with NFKD folding if non-Latin accent-insensitive search is requested.
- `find_ci` Unicode path is O(n·m) per start position (no allocation; correct for multi-char case maps). Revisit only if non-ASCII search shows up in profiles; full case-folding (ß↔ss) would need a folding crate and is a semantic change.
- `Query::score()` compiles the matcher per call; engines use `matcher()` once per search. Kept for test/external convenience.
- `egui_expressive` (forked) drags `vtracer`/`visioncortex`/`image` into the GUI even with default features off; trimming requires changing the fork.
- Streaming builds hold no live set past one chunk, but rayon sort scratch stays retained in thread arenas (~7 GiB RSS observed on the 100M-record host; harmless there). Revisit only if a smaller host OOMs during builds: cap sort threads or sort serially.

## Launch weight (2026-10-06)

- **The 100 MB resource ceiling is not established.** The latest 2026-10-09 sample of the GUI and complete live service measured 94,755 KiB PSS and 181,936 KiB summed RSS; the busy Btrfs sweep used about 59% of one core. An earlier sample exceeded 100 MB even in PSS. See `docs/feedback/production-readiness-validation-2026-10-09.md`. Repay by profiling the sweep under sustained writes, reducing GUI/renderer allocations, and recording peak combined memory and idle/active CPU on the user's normal graphics backend, including cold ingest and typed queries.
- **Windows compact snapshots still own the full base bytes.** `compact.rs` uses `std::fs::read` on Windows, so the Linux mmap measurements do not establish a Windows memory ceiling. Repay with a coherent bounded file-backed snapshot and platform tests during index replacement.

- **AccessKit is off.** `eframe` is built without `accesskit` because `accesskit_unix` panics (abort) when the AT-SPI socket refuses the connection, which stopped the window from opening. Screen readers get no tree until this is repaid. Unblock: an eframe/accesskit release that handles a missing a11y bus, then re-enable the feature.
- **Preparing complete browse orders is expensive.** The disk catalog now supports all eight ordinary sort directions, but building it for the 70,317,612-record index took 1,153 seconds and 70,810,034,176 bytes. Preparation is a separate background phase after index publication. Repay by integrating sort-key emission with metadata ingest and reducing duplicated path/name keys; measure cold preparation and disk footprint before replacing this design.
- **Typed searches still stream the compact base.** Complete browse orders cover term-free listings; text and regular-expression searches use the interruptible compact search. Repay with a disk candidate index that preserves name/path, Unicode, whole-word and accent matching, then measure uncommon queries on the full 70M-record index.
- **Timestamps past 2100 sort as unknown.** `matcher.rs` ignores corrupt future mtimes (found on crate files dated 2111) so they cannot pin the top of Modified order. A genuine file dated beyond 2100 would sort as the oldest.
- **Full metadata ingest retains sort scratch.** Automatic periodic rebuilds are disabled; first-run and explicit rebuild still need a measured small-memory ingest path. Repay by bounding spill-sort scratch and validating peak RSS during a cold full metadata build.
- **Btrfs sweeps can miss changes between ingest and watcher attachment.** The sweep cursor must be bound to filesystem generations captured before ingest, rather than a later attachment point. Repay by persisting per-subvolume ingest generations alongside the published index and replaying from them on first watcher attachment.
- **Btrfs reconciliation still materializes each sweep plan and folder subtree.** A removal/rename over two million descendants now fails before partial changes and marks the index stale; it no longer silently leaves ghost rows. Repay with durable prefix removals/moves and streamed bounded reconciliation, tested against concurrent writes and crash recovery.
- **NTFS mounted through FUSE has limited live events.** Saved-file metadata updates arrive through the native mount mark, but external renames and deletes need an NTFS journal reader. The UI reports the limitation. Repay with generation-bound USN journal updates and tests on the mounted NTFS drive, including overflow and offline/reconnect recovery.
- **Unmounted nested Btrfs subvolumes need metadata coverage.** Mounted subvolumes are indexed independently. Repay by resolving subvolume roots through native metadata and proving coverage without directory enumeration.
- **New mounts are discovered at service startup.** `watch_session.rs` captures the mount list once; attaching watchers and ingesting a drive mounted later are not implemented. Repay with mount-change detection, native metadata ingest and generation-bound watch attachment, tested across unmount/reconnect without disrupting other drives.

## Explorer interaction completion (2026-10-09)

- **External drag-out and multiple selection are unfinished.** The shared menus support open, reveal, rename, trash/undo, clipboard transfers and internal/native drop-in, but dragging to another application needs a native drag source and selection needs Ctrl/Shift semantics. Repay by implementing those platform interactions and testing both directions against a file manager on an owned virtual display.
- **File-format maps and ball graphs show bounded samples.** The file-format view displays up to 2,048 largest/smallest matching files; the graph displays at most 128 cached nodes. Their labels expose those limits. Repay with interactive drill-down that preserves meaningful allocation totals without materializing all files.
