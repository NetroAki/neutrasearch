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

- Accent folding covers Latin diacritics only (`strip_accent` in `neutra-core/src/matcher.rs`); other scripts still compare exactly. Repay with NFKD folding if non-Latin accent-insensitive search is requested.
- `find_ci` Unicode path is O(n·m) per start position (no allocation; correct for multi-char case maps). Revisit only if non-ASCII search shows up in profiles; full case-folding (ß↔ss) would need a folding crate and is a semantic change.
- `Query::score()` compiles the matcher per call; engines use `matcher()` once per search. Kept for test/external convenience.
- `egui_expressive` (forked) drags `vtracer`/`visioncortex`/`image` into the GUI even with default features off; trimming requires changing the fork.
- Streaming builds hold no live set past one chunk, but rayon sort scratch stays retained in thread arenas (~7 GiB RSS observed on the 100M-record host; harmless there). Revisit only if a smaller host OOMs during builds: cap sort threads or sort serially.
