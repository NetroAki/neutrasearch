# Debt ledger

Deliberate, recorded shortcuts and gaps. Repay conditions are concrete; entries are removed when repaid. See also the CHANGELOG for what was already repaid.

## Unwired public API

- **`DirectorySummaryOverlay` (live folder totals) has no production caller.** Built, generation-bound, and covered by four tests in `neutra-core/src/dir_summary.rs`, but every binary serves directory totals from the static `.dirs` sidecar (GUI treemap). The helper holds `base + delta` and could serve fresh totals through it.
  Unblock: a protocol message ("give me directory summary for path") served by the helper from the overlay, consumed by the GUI folder views; or the GUI holding a delta session.
- RESOLVED 2026-09-14 (removed): the ZFS-diff wiring question was decided — fanotify is the update path for every filesystem including ZFS (a watcher session on a ZFS mount works exactly like ext4/btrfs); there is no ZFS-specific update mode to build. `parse_diff` and the snapshot command builders remain public, tested API for external tooling.

## Benchmarks (this machine, 2026-09-19)

Two-device Btrfs RAID0 (nvme0n1p2 + nvme1n1p1, 3.6 TiB total): `@` tree 3.13M records in 34.2 s cold / 2.42 s warm (~1.29M records/s warm); `@home` tree 73.05M records in 491 s cold (~149k records/s). RAID0 needs no special handling: Btrfs metadata trees are filesystem-global, so TREE_SEARCH on any mount enumerates files striped across all member devices — coverage is per-subvolume, not per-device, and the mount selection already scans every mounted subvolume. Observation: the last shard (objectid ≥128M, 38.4M nodes) dominates `@home` scan time (485 s of 491 s wall) — worth revisiting shard balance if that tree matters.

## ZFS

- **The interim lane is first-class (2026-09-14)**: `neutrasearch index --zfs-enumerate` requests the single-pass enumeration as a scan parameter (protocol v9 `allow_zfs_enumerate`), which survives pkexec's environment stripping — the env var remains for direct helper use. `neutrasearch-helper --zfs-probe` reports which lanes a machine has (`zfs version`, libzpool soname, walk availability). It is still a directory walk: opt-in, xdev- and symlink-safe, fstatat per entry.
- **`libzpool` ZAP backend remains a placeholder** (feature `zfs-libzpool`). Doing it properly is a project, not a patch:
  1. Pin an OpenZFS release (`OPENZFS_REV`, e.g. `zfs-2.3.x`) in-repo; everything derives from the pin.
  2. Generate FFI with bindgen against that tree's headers (`sys/dmu.h`, `sys/zap.h`, `sys/sa.h`, `sys/zfs_znode.h`, `sys/dnode.h`) with a strict symbol allowlist — never hand-written, because the internal signatures shifted across 2.1→2.2.
  3. Link against the pinned tree's libzpool: a userspace DMU that reads vdevs directly, no kernel module needed on the indexing host.
  4. Traversal: objset → master node (object 1) → ZPL root directory (object 3) → recursive ZAP walk (`zap_cursor`) mapping name→object; per object `dmu_bonus_hold` for `znode_phys` (times, generation, rdev) **plus SA attribute lookups for mode/uid/gid/size** — modern pools keep those in the SA table, and the SA framework is the part that demands generated bindings.
  5. Runtime ABI gate: probe (open objset, read master node) before emitting records; mismatch fails closed with `--zfs-probe`-style diagnostics.
  6. Test on a file-vdev pool (libzpool opens them userspace-style, like zdb) inside a privileged CI container with zfsutils; populate a known tree and assert records.
  Unblock: an OpenZFS checkout + a privileged CI job. Estimated as a focused project (the traversal is small; the pinned-tree CI and SA handling are the work).

## Oversized modules (from the repo pre-scan; do not grow further)

Splits repaid 2026-09-14: `neutra-query` (main + service), `neutra-mcp` (main + store + policy + tools), `neutra-ntfs` (lib + geometry + records), `neutra-helper` (main + store + protocol + scan), `neutra-gui` (main + transport), `neutra-macos` (lib + bulk).

Still oversized — split at the next feature that touches them, don't split speculatively:
- `neutra-gui/src/ui.rs` (~1.9k lines): toolbar/menu/banner chrome vs `results.rs`/`treemap.rs` views; extract the shared widget + format helpers into `ui/widgets.rs` when touched.
- `neutra-gui/src/main.rs` (~1.6k lines): `NeutraApp` event handling and scan orchestration could become `app.rs` behind the eframe trait.
- `neutra-core/src/dir_summary.rs` (~1.3k) and `compact.rs` (~1.2k): on-disk format code is cohesive; split only if a third concern appears.

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

- **AccessKit is off.** `eframe` is built without `accesskit` because `accesskit_unix` panics (abort) when the AT-SPI socket refuses the connection, which stopped the window from opening. Screen readers get no tree until this is repaid. Unblock: an eframe/accesskit release that handles a missing a11y bus, then re-enable the feature.
- **After a GUI-run scan the first screen waits about 15 seconds for the sorted lists.** The GUI publishes the final index itself and then builds `<index>.rank` in the background (about 130 CPU-seconds on 70M records, two full passes). Repay by computing both orderings in one pass over the blocks.
- **Leader lists cover two orderings only.** Name and path orderings, text queries and filters that leave fewer than a page among the leaders run the full search. Repay with per-block min/max tables if those need to be instant.
- **Timestamps past 2100 sort as unknown.** `matcher.rs` ignores corrupt future mtimes (found on crate files dated 2111) so they cannot pin the top of Modified order. A genuine file dated beyond 2100 would sort as the oldest.
- **Full scans still peak high in the helper.** A full rebuild peaked at 16 GB earlier; scans now run only from the Scan button, the weekly timer, or the first run. Unblock: cap sort threads in the helper (see the sort scratch note above).
- **Btrfs live updates are two layers.** Closed writes on the user's home mount arrive within a second through `neutra-helper/src/watch_mount.rs` (the kernel refuses filesystem-wide fanotify marks on subvolume mounts). Everything else, including deletes, renames, new folders and the other subvolumes, arrives within about 20 seconds through `neutra-helper/src/watch_sweep.rs`, which asks each subvolume what changed since its last transaction id (`neutra-btrfs/src/linux/sweep.rs`) and reconciles those folders against the index (`sweep_plan.rs`). Known edges:
  - A sweep position older than the current base is not trusted, so changes in the minute around a weekly rebuild or a compaction can be missed until the next rebuild. Repay by having scans record the filesystem generation at their start.
  - The sweep assumes a subvolume is mounted at its own root. A mount of a subdirectory would produce wrong paths. Repay by reading the mount's root from mountinfo and prefixing it.
  - Nested subvolumes keep their name in the parent but are only followed when they are mounted themselves.
  - Hard links and extended back references (`INODE_EXTREF`) use the first parent only.
  - Folders busy in more than four of the last eight sweeps, and build or cache directories named in `NOISY_DIRS`, are skipped to keep the log near 20 MB a day. Their changes arrive once they settle or at the weekly rebuild.
  - A removed folder with more than two million entries is dropped by name only. Repay with a prefix tombstone in the delta log.
