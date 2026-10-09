//! neutrasearch-helper: the privileged (or platform-native) scanning daemon.
//!
//! Speaks neutra-core::proto over stdin/stdout (framed bincode). The same
//! binary is auto-provisioned onto Linux/Windows/macOS file servers, so all
//! logging goes to stderr — stdout is protocol-only.
//!
//! Lanes per platform (no filesystem walking anywhere):
//!   linux   btrfs (TREE_SEARCH ioctl) · ext4 (libext2fs raw device) ·
//!           ntfs (raw $MFT parse) · zfs (snapshot+ZAP, experimental)
//!   windows ntfs ($MFT via volume handle)
//!   macos   Spotlight index (primary) · getattrlistbulk (labeled fallback)

mod protocol;
mod scan;
mod store;
#[cfg(target_os = "linux")]
mod sweep_plan;
#[cfg(target_os = "linux")]
mod watch_linux;
#[cfg(target_os = "linux")]
mod watch_mount;
#[cfg(target_os = "linux")]
mod watch_process;
#[cfg(target_os = "linux")]
mod watch_session;
#[cfg(target_os = "linux")]
mod watch_sweep;
use protocol::run_protocol;
use scan::{dispatch_lane, exclusion_prefixes, find_local_mount, path_has_component_prefix};
#[cfg(test)]
use store::write_compaction_marker;
#[cfg(test)]
use store::DurableStore;
use store::{acquire_rebuild_lock, sync_parent};
#[cfg(target_os = "windows")]
mod windows_service;
use anyhow::{Context, Result};
use neutra_core::proto::HELPER_BUILD;
use neutra_core::CompactIndex;
#[cfg(test)]
use std::io::{Cursor, Write};
use std::time::Duration;

/// Watched-event bursts (builds, checkouts) commit once after this quiet window.
pub(crate) const WATCH_DEBOUNCE: Duration = Duration::from_millis(250);
pub(crate) const MAX_PENDING_CHANGES: usize = crate::scan::MAX_DELTA_CHANGES;

fn main() -> Result<()> {
    #[cfg(target_os = "linux")]
    unsafe {
        libc::mallopt(libc::M_ARENA_MAX, 2);
    }
    std::env::set_var("RAYON_NUM_THREADS", "2");
    #[cfg(target_os = "linux")]
    if std::env::args().nth(1).as_deref() == Some("--watch-events") {
        return watch_process::run();
    }
    #[cfg(target_os = "windows")]
    if std::env::args().nth(1).as_deref() == Some("--windows-service") {
        return windows_service::run();
    }

    // `--version`/`--build` are used by auto-provisioning to decide whether a
    // remote copy is stale. Keep them dependency-free and instant.
    let arg = std::env::args().nth(1);
    let watch_mount = if arg.as_deref() == Some("--watch-index") {
        Some((
            std::path::PathBuf::from(
                std::env::args()
                    .nth(3)
                    .context("internal usage: --watch-index INDEX.nsx MOUNT [SOURCE]")?,
            ),
            std::env::args()
                .nth(4)
                .map(|source| source.parse::<u32>().context("invalid source ID"))
                .transpose()?
                .unwrap_or(0),
        ))
    } else {
        None
    };
    let serve_index = if matches!(arg.as_deref(), Some("--serve-index" | "--watch-index")) {
        Some(std::path::PathBuf::from(std::env::args().nth(2).context(
            "internal usage: neutrasearch-helper --serve-index INDEX.nsx",
        )?))
    } else {
        std::env::var_os("NEUTRASEARCH_SERVE_INDEX").map(std::path::PathBuf::from)
    };
    match arg.as_deref() {
        Some("--version") | Some("-V") => {
            println!(
                "neutrasearch-helper {} build {}",
                env!("CARGO_PKG_VERSION"),
                HELPER_BUILD
            );
            return Ok(());
        }
        Some("--build") => {
            println!("{HELPER_BUILD}");
            return Ok(());
        }
        Some("--serve-index") | Some("--watch-index") => {}
        Some("--zfs-probe") => {
            let probe = neutra_zfs::probe();
            println!(
                "zfs_cli={} libzpool={}",
                probe.zfs_cli_version.as_deref().unwrap_or("not installed"),
                probe.libzpool_soname.unwrap_or("not found"),
            );
            println!("initial indexing requires a verified native ZAP backend; no directory fallback is available");
            return Ok(());
        }
        Some("--scan-summary") => {
            let target = std::env::args()
                .nth(2)
                .context("use: neutrasearch-helper --scan-summary MOUNT")?;
            let mount = find_local_mount(&target)?;
            let mut received = 0u64;
            let stats = dispatch_lane(&mount, &mut |_| received += 1)?;
            println!(
                "fs={} mount={} records={} emitted={} files={} dirs={} wall_ms={} detail={}",
                mount.fs.label(),
                mount.mountpoint.display(),
                stats.records,
                received,
                stats.files,
                stats.dirs,
                stats.wall_ms,
                stats.detail
            );
            return Ok(());
        }
        Some("--build-index") => {
            let positional: Vec<_> = std::env::args().skip(2).collect();
            let [target, output] = positional.as_slice() else {
                anyhow::bail!("use: neutrasearch index MOUNT --output INDEX.nsx");
            };
            let output = std::path::PathBuf::from(output);
            let mount = find_local_mount(target)?;
            let (_rebuild_lock, delta_path) = acquire_rebuild_lock(&output)?;
            let mountpoint = mount.mountpoint.clone();
            let exclusions = exclusion_prefixes(&mountpoint);
            // Stream scan batches straight into the spill: one huge mount
            // materialized here peaked past 25 GiB.
            let mut spill = neutra_core::SpillAccumulator::begin(&output)?;
            let mut spill_error: Option<std::io::Error> = None;
            let scan = dispatch_lane(&mount, &mut |record| {
                if spill_error.is_some() {
                    return;
                }
                if !exclusions
                    .iter()
                    .any(|prefix| path_has_component_prefix(record.path.as_ref(), prefix))
                {
                    if let Err(error) = spill.push_batch(vec![record]) {
                        spill_error = Some(error);
                    }
                }
            })?;
            if let Some(error) = spill_error {
                anyhow::bail!("spill scan batches: {error}");
            }
            let built = CompactIndex::rebuild_streamed(spill.finish()?, &output)?;
            match std::fs::remove_file(&delta_path) {
                Ok(()) => sync_parent(&delta_path)?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error).context("remove obsolete delta WAL after rebuild"),
            }
            println!("fs={} mount={} records={} scan_ms={} index_bytes={} blocks={} trigrams={} build_ms={} output={}",mount.fs.label(),mount.mountpoint.display(),scan.records,scan.wall_ms,built.bytes,built.blocks,built.trigrams,built.wall_ms,output.display());
            return Ok(());
        }
        _ => {}
    }

    let serve_index = serve_index
        .map(|path| {
            std::fs::canonicalize(&path)
                .with_context(|| format!("resolve compact index {}", path.display()))
        })
        .transpose()?;
    let watch_mount = watch_mount
        .map(|(path, source)| {
            std::fs::canonicalize(&path)
                .with_context(|| format!("resolve watched mount {}", path.display()))
                .map(|path| (path, source))
        })
        .transpose()?;

    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "neutrasearch_helper=info,neutra_helper=info".into()),
        )
        .init();

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut rin = stdin.lock();
    run_protocol(&mut rin, Box::new(stdout), serve_index, watch_mount, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::run_protocol_with_auth;
    use crate::scan::{
        parse_macos_mount_output, portable_path_in_root, resolve_scan_mounts, run_bounded,
        validate_delta_changes, validate_query, validate_scan_roots, MAX_QUERY_RESULTS,
    };
    use crate::store::{
        append_suffix, compaction_marker, compaction_marker_temp, compaction_stage,
    };
    use neutra_core::mounts::MountInfo;
    use neutra_core::proto::{read_frame, write_frame, ClientMsg, HelperMsg, PROTO_VERSION};
    use neutra_core::{DeltaChange, FileKind, FileRecord, FsKind, Query};
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    fn record(path: &str, size: u64) -> FileRecord {
        FileRecord {
            path: path.into(),
            size,
            mtime: size as i64,
            mode: 0,
            kind: FileKind::File,
            fs: FsKind::Btrfs,
            native_id: size,
            native_parent: 1,
            source: 0,
            disk: size,
        }
    }

    fn store_paths(label: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let base = std::env::temp_dir().join(format!(
            "neutrasearch-helper-{label}-{}.nsx",
            std::process::id()
        ));
        let mut delta = base.clone();
        delta.set_extension("delta");
        remove_store(&base, &delta);
        (base, delta)
    }

    fn build_test_base(
        records: &[FileRecord],
        path: &std::path::Path,
    ) -> anyhow::Result<neutra_core::CompactBuildStats> {
        let mut spill = neutra_core::SpillAccumulator::begin(path)?;
        spill.push_batch(records.to_vec())?;
        Ok(CompactIndex::rebuild_streamed(spill.finish()?, path)?)
    }

    fn remove_store(base: &std::path::Path, delta: &std::path::Path) {
        let mut lock = delta.as_os_str().to_os_string();
        lock.push(".lock");
        for path in [
            base.to_path_buf(),
            delta.to_path_buf(),
            lock.into(),
            append_suffix(base, ".new"),
            compaction_stage(base),
            append_suffix(&compaction_stage(base), ".new"),
            compaction_marker(base),
            compaction_marker_temp(base),
        ] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_ready_catalog_checkpoints_changes_without_rewriting_the_base() {
        let (base_path, delta_path) = store_paths("catalog-checkpoint");
        build_test_base(&[record("/old.txt", 1)], &base_path).unwrap();
        let base = CompactIndex::open_fast(&base_path).unwrap();
        let generation = base.generation();
        neutra_core::BrowserIndex::ensure(&base_path, generation).unwrap();
        let mut store = DurableStore::open_with_threshold(&base_path, 17).unwrap();
        let result = store
            .apply_bounded(vec![neutra_core::DeltaChange::Upsert(record(
                "/saved.txt",
                9,
            ))])
            .unwrap();
        assert!(result.compacted.is_none());
        assert_eq!(
            CompactIndex::generation_on_disk(&base_path).unwrap(),
            generation
        );
        let browser = neutra_core::BrowserIndex::open(&base_path, generation).unwrap();
        assert!(browser.covers_delta(&base_path).unwrap());
        store
            .delta
            .apply(neutra_core::DeltaChange::Upsert(record("/pending.txt", 10)))
            .unwrap();
        store.delta.sync().unwrap();
        assert!(!browser.covers_delta(&base_path).unwrap());
        store
            .apply_bounded(vec![neutra_core::DeltaChange::Upsert(record(
                "/pending.txt",
                10,
            ))])
            .unwrap();
        assert!(browser.covers_delta(&base_path).unwrap());
        let hits = browser
            .search(&base, &neutra_core::Query::default(), None)
            .unwrap()
            .0;
        assert!(hits.iter().any(|hit| &*hit.record.path == "/saved.txt"));
        drop(browser);
        drop(base);
        drop(store);
        remove_store(&base_path, &delta_path);
        for suffix in [
            ".browse",
            ".browse.base",
            ".browse.lock",
            ".browse-wal",
            ".browse-shm",
        ] {
            let _ = std::fs::remove_file(store::append_suffix(&base_path, suffix));
        }
    }

    #[test]
    fn durable_store_syncs_and_searches_delta() {
        let (base_path, delta_path) = store_paths("store");
        build_test_base(&[record("/old.txt", 1)], &base_path).unwrap();

        let mut store = DurableStore::open(&base_path).unwrap();
        let applied = store
            .apply(vec![
                DeltaChange::Remove("/old.txt".into()),
                DeltaChange::Upsert(record("/new.txt", 2)),
            ])
            .unwrap();
        assert_eq!(applied.0, 2);
        let (hits, stats) = store.search(&Query::parse("ext:txt")).unwrap();
        assert_eq!(stats.matched, 1);
        assert_eq!(hits[0].record.path.as_ref(), "/new.txt");
        drop(store);

        let reopened = DurableStore::open(&base_path).unwrap();
        let (hits, _) = reopened.search(&Query::parse("new")).unwrap();
        assert_eq!(hits[0].record.path.as_ref(), "/new.txt");
        drop(reopened);
        remove_store(&base_path, &delta_path);
    }

    #[test]
    fn ignores_unpublished_partial_marker_and_staged_base() {
        let (base_path, delta_path) = store_paths("partial-marker");
        build_test_base(&[record("/old.txt", 1)], &base_path).unwrap();
        drop(DurableStore::open(&base_path).unwrap());
        let staged_path = compaction_stage(&base_path);
        build_test_base(&[record("/new.txt", 2)], &staged_path).unwrap();
        std::fs::write(compaction_marker_temp(&base_path), [1, 2, 3]).unwrap();

        let store = DurableStore::open(&base_path).unwrap();
        let (hits, stats) = store.search(&Query::parse("ext:txt")).unwrap();
        assert_eq!(stats.matched, 1);
        assert_eq!(hits[0].record.path.as_ref(), "/old.txt");
        assert!(!staged_path.exists());
        assert!(!compaction_marker_temp(&base_path).exists());
        drop(store);
        remove_store(&base_path, &delta_path);
    }

    #[test]
    fn recovers_compaction_after_marker_before_wal_reset() {
        let (base_path, delta_path) = store_paths("recover-before-reset");
        build_test_base(&[record("/old.txt", 1)], &base_path).unwrap();
        let mut store = DurableStore::open(&base_path).unwrap();
        store
            .apply(vec![
                DeltaChange::Remove("/old.txt".into()),
                DeltaChange::Upsert(record("/new.txt", 2)),
            ])
            .unwrap();
        drop(store);

        let staged_path = compaction_stage(&base_path);
        let built = build_test_base(&[record("/new.txt", 2)], &staged_path).unwrap();
        write_compaction_marker(&compaction_marker(&base_path), built.generation).unwrap();

        let recovered = DurableStore::open(&base_path).unwrap();
        let (hits, stats) = recovered.search(&Query::parse("ext:txt")).unwrap();
        assert_eq!(stats.matched, 1);
        assert_eq!(hits[0].record.path.as_ref(), "/new.txt");
        assert_eq!(recovered.delta.generation(), built.generation);
        assert!(!compaction_marker(&base_path).exists());
        drop(recovered);
        remove_store(&base_path, &delta_path);
    }

    #[test]
    fn recovers_compaction_after_wal_reset_before_base_publish() {
        let (base_path, delta_path) = store_paths("recover-after-reset");
        build_test_base(&[record("/old.txt", 1)], &base_path).unwrap();
        let mut store = DurableStore::open(&base_path).unwrap();
        store
            .apply(vec![
                DeltaChange::Remove("/old.txt".into()),
                DeltaChange::Upsert(record("/new.txt", 2)),
            ])
            .unwrap();
        let staged_path = compaction_stage(&base_path);
        let built = build_test_base(&[record("/new.txt", 2)], &staged_path).unwrap();
        write_compaction_marker(&compaction_marker(&base_path), built.generation).unwrap();
        store.delta.reset(built.generation).unwrap();
        drop(store);

        let recovered = DurableStore::open(&base_path).unwrap();
        let (hits, stats) = recovered.search(&Query::parse("ext:txt")).unwrap();
        assert_eq!(stats.matched, 1);
        assert_eq!(hits[0].record.path.as_ref(), "/new.txt");
        assert_eq!(recovered.delta.generation(), built.generation);
        assert!(!compaction_marker(&base_path).exists());
        drop(recovered);
        remove_store(&base_path, &delta_path);
    }

    #[test]
    fn recovers_when_marker_is_lost_with_a_torn_reset_wal() {
        for length in [0, 1, 7, 15] {
            let (base_path, delta_path) = store_paths(&format!("recover-marker-lost-{length}"));
            build_test_base(&[record("/old.txt", 1)], &base_path).unwrap();
            let mut store = DurableStore::open(&base_path).unwrap();
            store
                .apply(vec![
                    DeltaChange::Remove("/old.txt".into()),
                    DeltaChange::Upsert(record("/new.txt", 2)),
                ])
                .unwrap();
            let staged_path = compaction_stage(&base_path);
            let built = build_test_base(&[record("/new.txt", 2)], &staged_path).unwrap();
            let marker = compaction_marker(&base_path);
            write_compaction_marker(&marker, built.generation).unwrap();
            store.delta.reset(built.generation).unwrap();
            drop(store);
            std::fs::remove_file(marker).unwrap();
            std::fs::OpenOptions::new()
                .write(true)
                .open(&delta_path)
                .unwrap()
                .set_len(length)
                .unwrap();

            let recovered = DurableStore::open(&base_path).unwrap();
            let (hits, stats) = recovered.search(&Query::parse("ext:txt")).unwrap();
            assert_eq!(stats.matched, 1);
            assert_eq!(hits[0].record.path.as_ref(), "/new.txt");
            assert_eq!(recovered.delta.generation(), built.generation);
            drop(recovered);
            remove_store(&base_path, &delta_path);
        }
    }

    #[test]
    fn recovers_torn_wal_header_when_staged_base_is_verified() {
        for length in [0, 1, 7, 15] {
            let (base_path, delta_path) = store_paths(&format!("recover-torn-{length}"));
            build_test_base(&[record("/old.txt", 1)], &base_path).unwrap();
            let mut store = DurableStore::open(&base_path).unwrap();
            store
                .apply(vec![
                    DeltaChange::Remove("/old.txt".into()),
                    DeltaChange::Upsert(record("/new.txt", 2)),
                ])
                .unwrap();
            let staged_path = compaction_stage(&base_path);
            let built = build_test_base(&[record("/new.txt", 2)], &staged_path).unwrap();
            write_compaction_marker(&compaction_marker(&base_path), built.generation).unwrap();
            store.delta.reset(built.generation).unwrap();
            drop(store);
            std::fs::OpenOptions::new()
                .write(true)
                .open(&delta_path)
                .unwrap()
                .set_len(length)
                .unwrap();

            let recovered = DurableStore::open(&base_path).unwrap();
            let (hits, stats) = recovered.search(&Query::parse("ext:txt")).unwrap();
            assert_eq!(stats.matched, 1);
            assert_eq!(hits[0].record.path.as_ref(), "/new.txt");
            assert_eq!(recovered.delta.generation(), built.generation);
            drop(recovered);
            remove_store(&base_path, &delta_path);
        }
    }

    #[test]
    fn corrupt_complete_wal_is_not_discarded_during_compaction_recovery() {
        let (base_path, delta_path) = store_paths("reject-corrupt-recovery-wal");
        build_test_base(&[record("/old.txt", 1)], &base_path).unwrap();
        let mut store = DurableStore::open(&base_path).unwrap();
        let staged_path = compaction_stage(&base_path);
        let built = build_test_base(&[record("/new.txt", 2)], &staged_path).unwrap();
        write_compaction_marker(&compaction_marker(&base_path), built.generation).unwrap();
        store.delta.reset(built.generation).unwrap();
        store
            .delta
            .apply(DeltaChange::Upsert(record("/later.txt", 3)))
            .unwrap();
        drop(store);

        let mut wal = std::fs::read(&delta_path).unwrap();
        *wal.last_mut().unwrap() ^= 0xff;
        std::fs::write(&delta_path, wal).unwrap();
        let error = DurableStore::open(&base_path)
            .err()
            .expect("corrupt complete WAL must fail closed");
        assert!(format!("{error:#}").contains("checksum mismatch"));
        assert!(compaction_marker(&base_path).exists());

        remove_store(&base_path, &delta_path);
    }

    #[test]
    fn macos_mount_parser_keeps_user_visible_native_volumes() {
        let mounts = parse_macos_mount_output(
            "/dev/disk3s1s1 on / (apfs, sealed, local)\n\
             /dev/disk3s5 on /System/Volumes/Data (apfs, local)\n\
             /dev/disk7s1 on /Volumes/Archive Drive (hfs, local)\n\
             server:/share on /Volumes/Team (nfs, nodev)\n",
        );
        assert_eq!(mounts.len(), 2);
        assert_eq!(mounts[0].mountpoint, std::path::Path::new("/"));
        assert_eq!(
            mounts[1].mountpoint,
            std::path::Path::new("/Volumes/Archive Drive")
        );
        assert_eq!(mounts[1].fs, FsKind::Unsupported("hfs".into()));
    }

    #[test]
    fn scan_requests_use_trusted_mount_metadata() {
        let trusted = MountInfo {
            device: "/dev/trusted".into(),
            mountpoint: "/mnt/data".into(),
            fs: FsKind::Ext4,
            source: neutra_core::MountSource::Local,
        };
        let spoofed = MountInfo {
            device: "/dev/evil".into(),
            mountpoint: "/mnt/data".into(),
            fs: FsKind::Ntfs,
            source: neutra_core::MountSource::Local,
        };
        assert!(resolve_scan_mounts(Vec::new(), vec![trusted.clone()], true)
            .unwrap()
            .is_empty());
        let resolved = resolve_scan_mounts(vec![spoofed], vec![trusted], true).unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].device, "/dev/trusted");
        assert!(matches!(resolved[0].fs, FsKind::Ext4));
        assert!(resolve_scan_mounts(
            vec![MountInfo {
                mountpoint: "/unknown".into(),
                device: "/dev/evil".into(),
                fs: FsKind::Ntfs,
                source: neutra_core::MountSource::Local,
            }],
            Vec::new(),
            true,
        )
        .is_err());
    }

    #[test]
    fn bounded_call_returns_before_a_slow_operation_finishes() {
        let started = std::time::Instant::now();
        let result = run_bounded(
            "test operation",
            std::time::Duration::from_millis(10),
            || {
                std::thread::sleep(std::time::Duration::from_secs(1));
                42
            },
        );
        assert_eq!(result, None);
        assert!(started.elapsed() < std::time::Duration::from_millis(500));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_discovery_reports_only_real_ntfs_volume_roots() {
        let mounts = discover_local_mounts();
        assert!(
            !mounts.is_empty(),
            "Windows CI host should expose its system volume"
        );
        assert!(mounts.iter().all(|mount| mount.fs == FsKind::Ntfs));
        assert!(mounts.iter().all(|mount| mount.mountpoint.is_absolute()));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_discovery_includes_the_user_visible_root_volume() {
        let mounts = discover_local_mounts();
        assert!(
            mounts
                .iter()
                .any(|mount| mount.mountpoint == std::path::Path::new("/")),
            "macOS CI host should expose its root APFS volume"
        );
    }

    #[test]
    fn default_snapshot_directory_is_excluded_by_component() {
        let excluded = |mountpoint: &str, path: &str| {
            exclusion_prefixes(std::path::Path::new(mountpoint))
                .iter()
                .any(|prefix| path_has_component_prefix(path, prefix))
        };
        assert!(excluded("/", "/.snapshots/42/file"));
        assert!(excluded("/home", "/home/.snapshots/42/file"));
        assert!(!excluded("/", "/.snapshots-old/file"));
        assert!(excluded("/", "/proc/self/status"));
        assert!(excluded("/", "/sys/kernel"));
        assert!(!excluded("/home", "/home/system/file"));
    }

    #[test]
    fn approved_scan_roots_are_bounded_to_requested_mounts() {
        let (device, mountpoint, root, inside, sibling) = if cfg!(target_os = "windows") {
            (
                r"C:",
                r"C:\",
                r"C:\Users\alex\Documents",
                r"C:\Users\alex\Documents\report.pdf",
                r"C:\Users\alex\Documents-old\report.pdf",
            )
        } else {
            (
                "/dev/root",
                "/",
                "/home/alex/Documents",
                "/home/alex/Documents/report.pdf",
                "/home/alex/Documents-old/report.pdf",
            )
        };
        let mount = MountInfo {
            device: device.into(),
            mountpoint: mountpoint.into(),
            fs: FsKind::Btrfs,
            source: neutra_core::MountSource::Local,
        };
        let roots = validate_scan_roots(vec![root.into()], std::slice::from_ref(&mount)).unwrap();
        assert!(portable_path_in_root(inside, &roots[0]));
        assert!(!portable_path_in_root(sibling, &roots[0]));
        assert!(validate_scan_roots(Vec::new(), std::slice::from_ref(&mount)).is_err());
        assert!(validate_scan_roots(vec!["relative".into()], &[mount]).is_err());
    }

    #[test]
    fn one_shot_protocol_ends_after_scan_preparation_error() {
        use std::sync::Mutex;

        #[derive(Clone)]
        struct SharedOutput(Arc<Mutex<Vec<u8>>>);
        impl Write for SharedOutput {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let mut input = Vec::new();
        write_frame(
            &mut input,
            &ClientMsg::Hello {
                proto: PROTO_VERSION,
            },
        )
        .unwrap();
        write_frame(
            &mut input,
            &ClientMsg::Scan {
                mounts: vec![MountInfo {
                    device: "untrusted".into(),
                    mountpoint: "/not-a-trusted-mount".into(),
                    fs: FsKind::Ext4,
                    source: neutra_core::MountSource::Local,
                }],
                roots: vec!["/not-a-trusted-mount".into()],
                allow_zfs_enumerate: false,
            },
        )
        .unwrap();

        let bytes = Arc::new(Mutex::new(Vec::new()));
        run_protocol(
            &mut Cursor::new(input),
            Box::new(SharedOutput(Arc::clone(&bytes))),
            None,
            None,
            None,
        )
        .unwrap();

        let mut output = Cursor::new(bytes.lock().unwrap().clone());
        assert!(matches!(
            read_frame::<_, HelperMsg>(&mut output).unwrap(),
            Some(HelperMsg::Hello { .. })
        ));
        assert!(matches!(
            read_frame::<_, HelperMsg>(&mut output).unwrap(),
            Some(HelperMsg::Error(error)) if error.contains("not present in the trusted OS mount table")
        ));
    }

    #[test]
    fn directory_summary_serves_live_totals_from_the_durable_pair() {
        use std::sync::Mutex;

        #[derive(Clone)]
        struct SharedOutput(Arc<Mutex<Vec<u8>>>);
        impl Write for SharedOutput {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let base =
            std::env::temp_dir().join(format!("neutra-helper-dirsum-{}.nsx", std::process::id()));
        let _ = std::fs::remove_file(&base);
        let mut delta_path = base.clone();
        delta_path.set_extension("delta");
        let docs_dir = FileRecord {
            path: "/docs".into(),
            size: 0,
            mtime: 0,
            mode: 0,
            kind: FileKind::Dir,
            fs: FsKind::Btrfs,
            native_id: 0,
            native_parent: 0,
            source: 0,
            disk: 0,
        };
        let records = vec![docs_dir, record("/docs/a.txt", 10)];
        build_test_base(&records, &base).unwrap();
        let mut input = Vec::new();
        write_frame(
            &mut input,
            &ClientMsg::Hello {
                proto: PROTO_VERSION,
            },
        )
        .unwrap();
        write_frame(
            &mut input,
            &ClientMsg::ApplyDelta {
                changes: vec![DeltaChange::Upsert(record("/docs/b.txt", 5))],
            },
        )
        .unwrap();
        write_frame(
            &mut input,
            &ClientMsg::DirectorySummary {
                source: 0,
                path: "/docs".into(),
            },
        )
        .unwrap();
        write_frame(&mut input, &ClientMsg::Shutdown).unwrap();

        let bytes = Arc::new(Mutex::new(Vec::new()));
        run_protocol(
            &mut Cursor::new(input),
            Box::new(SharedOutput(Arc::clone(&bytes))),
            Some(base.clone()),
            None,
            None,
        )
        .unwrap();

        let mut output = Cursor::new(bytes.lock().unwrap().clone());
        let mut entry = None;
        while let Ok(Some(msg)) = read_frame::<_, HelperMsg>(&mut output) {
            if let HelperMsg::DirectorySummary { entry: found } = msg {
                entry = found;
            }
        }
        let entry = entry.expect("directory summary response");
        assert_eq!(entry.logical_bytes, 15);
        assert_eq!(entry.file_count, 2);
        store_cleanup(&base, &delta_path);
    }

    fn store_cleanup(base: &std::path::Path, delta: &std::path::Path) {
        let mut lock = delta.as_os_str().to_os_string();
        lock.push(".lock");
        for path in [
            base.to_path_buf(),
            delta.to_path_buf(),
            std::path::PathBuf::from(lock),
        ] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn authentication_runs_after_hello_before_commands() {
        use std::sync::Mutex;

        #[derive(Clone)]
        struct SharedOutput(Arc<Mutex<Vec<u8>>>);
        impl Write for SharedOutput {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let mut input = Vec::new();
        write_frame(
            &mut input,
            &ClientMsg::Hello {
                proto: PROTO_VERSION,
            },
        )
        .unwrap();
        write_frame(&mut input, &ClientMsg::Shutdown).unwrap();

        let bytes = Arc::new(Mutex::new(Vec::new()));
        let authenticated = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let callback_authenticated = Arc::clone(&authenticated);
        let callback_output = Arc::clone(&bytes);
        let authenticate = move || {
            let mut output = Cursor::new(callback_output.lock().unwrap().clone());
            assert!(matches!(
                read_frame::<_, HelperMsg>(&mut output).unwrap(),
                Some(HelperMsg::Hello { .. })
            ));
            callback_authenticated.store(true, Ordering::Release);
            Ok(())
        };

        run_protocol_with_auth(
            &mut Cursor::new(input),
            Box::new(SharedOutput(Arc::clone(&bytes))),
            None,
            None,
            None,
            Some(&authenticate),
        )
        .unwrap();
        assert!(authenticated.load(Ordering::Acquire));
    }

    #[test]
    fn protocol_work_is_bounded_before_execution() {
        let mut query = Query::parse("needle");
        query.limit = 0;
        assert!(validate_query(&query).is_err());
        query.limit = MAX_QUERY_RESULTS + 1;
        assert!(validate_query(&query).is_err());
        query.limit = 1;
        query.scope_roots = vec!["relative/scope".into()];
        assert!(validate_query(&query).is_err());
        assert!(validate_delta_changes(&[DeltaChange::Remove("relative/path".into())]).is_err());
        assert!(
            validate_delta_changes(&[DeltaChange::Remove("/allowed/../secret".into())]).is_err()
        );
        assert!(resolve_scan_mounts(Vec::new(), Vec::new(), false).is_err());
    }

    #[test]
    fn durable_store_compacts_base_and_resets_delta_generation() {
        let (base_path, delta_path) = store_paths("compact");
        build_test_base(&[record("/old.txt", 1)], &base_path).unwrap();
        let original_generation = CompactIndex::open(&base_path).unwrap().generation();

        let mut store = DurableStore::open_with_threshold(&base_path, 17).unwrap();
        let applied = store
            .apply_bounded(vec![
                DeltaChange::Remove("/old.txt".into()),
                DeltaChange::Upsert(record("/new.txt", 2)),
            ])
            .unwrap();
        assert_eq!(applied.changes, 2);
        assert_eq!(applied.wal_bytes, 16);
        assert!(applied.compacted.is_some());
        let replacement_generation = store.base.as_ref().unwrap().generation();
        assert_ne!(replacement_generation, original_generation);
        assert_eq!(store.delta.generation(), replacement_generation);
        assert_eq!(store.delta.change_count().unwrap(), 0);
        let (hits, stats) = store.search(&Query::parse("ext:txt")).unwrap();
        assert_eq!(stats.matched, 1);
        assert_eq!(hits[0].record.path.as_ref(), "/new.txt");
        drop(store);

        let reopened = DurableStore::open(&base_path).unwrap();
        let (hits, stats) = reopened.search(&Query::parse("ext:txt")).unwrap();
        assert_eq!(stats.matched, 1);
        assert_eq!(hits[0].record.path.as_ref(), "/new.txt");
        drop(reopened);
        remove_store(&base_path, &delta_path);
    }
}
