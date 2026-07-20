//! neutra-helper: the privileged (or platform-native) scanning daemon.
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

use anyhow::{Context, Result};
use neutra_core::mounts::{FsKind, MountInfo};
use neutra_core::proto::{
    read_frame, write_frame, ClientMsg, HelperMsg, HELPER_BUILD, PROTO_VERSION,
};
use neutra_core::{
    CompactIndex, DeltaChange, DeltaIndex, FileRecord, Index, Query, ScanStats, SearchHit,
    SearchStats,
};
use std::io::{BufWriter, Stdout};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

const RECORD_BATCH: usize = 4096;

struct DurableStore {
    base: CompactIndex,
    delta: DeltaIndex,
}

impl DurableStore {
    fn open(path: &std::path::Path) -> Result<Self> {
        let base = CompactIndex::open(path)
            .with_context(|| format!("open compact index {}", path.display()))?;
        let mut delta_path = path.to_path_buf();
        delta_path.set_extension("delta");
        let delta = DeltaIndex::open(&delta_path, base.generation())
            .with_context(|| format!("open delta index {}", delta_path.display()))?;
        Ok(Self { base, delta })
    }

    fn search(&self, query: &Query) -> Result<(Vec<SearchHit>, SearchStats)> {
        Ok(self.base.search_with_delta(query, &self.delta)?)
    }

    fn apply(&mut self, changes: Vec<DeltaChange>) -> Result<(u32, u64, bool)> {
        let count = u32::try_from(changes.len()).context("delta batch is too large")?;
        for change in changes {
            self.delta.apply(change)?;
        }
        self.delta.sync()?;
        Ok((count, self.delta.wal_bytes(), self.delta.needs_compaction()))
    }
}

fn main() -> Result<()> {
    // `--version`/`--build` are used by auto-provisioning to decide whether a
    // remote copy is stale. Keep them dependency-free and instant.
    let arg = std::env::args().nth(1);
    let serve_index = if arg.as_deref() == Some("--serve-index") {
        Some(std::path::PathBuf::from(
            std::env::args()
                .nth(2)
                .context("usage: neutra-helper --serve-index INDEX.nsx")?,
        ))
    } else {
        std::env::var_os("NEUTRA_SERVE_INDEX").map(std::path::PathBuf::from)
    };
    match arg.as_deref() {
        Some("--version") | Some("-V") => {
            println!(
                "neutra-helper {} build {}",
                env!("CARGO_PKG_VERSION"),
                HELPER_BUILD
            );
            return Ok(());
        }
        Some("--build") => {
            println!("{HELPER_BUILD}");
            return Ok(());
        }
        Some("--serve-index") => {}
        Some("--scan-summary") => {
            let target = std::env::args()
                .nth(2)
                .context("usage: neutra-helper --scan-summary MOUNTPOINT")?;
            #[cfg(target_os = "linux")]
            {
                let mount = neutra_core::mounts::system_mounts()?
                    .into_iter()
                    .find(|m| m.mountpoint == std::path::Path::new(&target))
                    .with_context(|| format!("no supported mount at {target}"))?;
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
            #[cfg(not(target_os = "linux"))]
            anyhow::bail!("--scan-summary mount discovery is currently available on Linux");
        }
        Some("--build-index") => {
            let target = std::env::args()
                .nth(2)
                .context("usage: neutra-helper --build-index MOUNTPOINT OUTPUT")?;
            let output = std::path::PathBuf::from(
                std::env::args()
                    .nth(3)
                    .context("usage: neutra-helper --build-index MOUNTPOINT OUTPUT")?,
            );
            #[cfg(target_os = "linux")]
            {
                let mount = neutra_core::mounts::system_mounts()?
                    .into_iter()
                    .find(|m| m.mountpoint == std::path::Path::new(&target))
                    .with_context(|| format!("no supported mount at {target}"))?;
                let mut records = Vec::new();
                let scan = dispatch_lane(&mount, &mut |record| records.push(record))?;
                let built = CompactIndex::build(&records, &output)?;
                println!("fs={} mount={} records={} scan_ms={} index_bytes={} blocks={} trigrams={} build_ms={} output={}",mount.fs.label(),mount.mountpoint.display(),scan.records,scan.wall_ms,built.bytes,built.blocks,built.trigrams,built.wall_ms,output.display());
                return Ok(());
            }
            #[cfg(not(target_os = "linux"))]
            anyhow::bail!("--build-index mount discovery is currently available on Linux");
        }
        _ => {}
    }

    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "neutra_helper=info".into()),
        )
        .init();

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut rin = stdin.lock();
    let out = Arc::new(Mutex::new(BufWriter::new(stdout)));

    // Expect Hello first.
    let hello: Option<ClientMsg> = read_frame(&mut rin).context("reading Hello")?;
    match hello {
        Some(ClientMsg::Hello { proto }) if proto == PROTO_VERSION => {
            send(
                &out,
                &HelperMsg::Hello {
                    proto: PROTO_VERSION,
                    build: HELPER_BUILD,
                    os: std::env::consts::OS.to_string(),
                    arch: std::env::consts::ARCH.to_string(),
                },
            )?;
        }
        Some(ClientMsg::Hello { proto }) => {
            send(
                &out,
                &HelperMsg::Error(format!(
                    "protocol mismatch: client={proto} helper={PROTO_VERSION}"
                )),
            )?;
            return Ok(());
        }
        _ => {
            send(&out, &HelperMsg::Error("expected Hello".into()))?;
            return Ok(());
        }
    }

    // Scans populate one resident index; searches never trigger a rescan.
    let index = Arc::new(RwLock::new(Index::default()));
    let durable = serve_index
        .map(|path| DurableStore::open(&path).map(|store| Arc::new(RwLock::new(store))))
        .transpose()?;
    let mut scan_threads = Vec::new();
    loop {
        let msg: Option<ClientMsg> = read_frame(&mut rin).context("reading command")?;
        match msg {
            None | Some(ClientMsg::Shutdown) => break,
            Some(ClientMsg::Hello { .. }) => {
                send(&out, &HelperMsg::Error("duplicate Hello".into()))?;
            }
            Some(ClientMsg::Scan { mounts }) => launch_scans(mounts, &out, None, &mut scan_threads),
            Some(ClientMsg::ScanResident { mounts }) => {
                launch_scans(mounts, &out, Some(&index), &mut scan_threads)
            }
            Some(ClientMsg::Search { query }) => {
                let (hits, stats) = if let Some(store) = &durable {
                    store.read().unwrap().search(&query)?
                } else {
                    index.read().unwrap().search(&query)
                };
                send(
                    &out,
                    &HelperMsg::SearchResult {
                        hits: hits.into_iter().map(|hit| hit.record).collect(),
                        wall_us: stats.wall_us,
                    },
                )?;
            }
            Some(ClientMsg::ApplyDelta { changes }) => {
                let Some(store) = &durable else {
                    send(
                        &out,
                        &HelperMsg::Error(
                            "ApplyDelta requires neutra-helper --serve-index INDEX.nsx".into(),
                        ),
                    )?;
                    continue;
                };
                let (changes, wal_bytes, needs_compaction) =
                    store.write().unwrap().apply(changes)?;
                send(
                    &out,
                    &HelperMsg::DeltaApplied {
                        changes,
                        wal_bytes,
                        needs_compaction,
                    },
                )?;
            }
        }
    }

    for t in scan_threads {
        let _ = t.join();
    }
    Ok(())
}

fn launch_scans(
    mounts: Vec<MountInfo>,
    out: &Arc<Mutex<BufWriter<Stdout>>>,
    index: Option<&Arc<RwLock<Index>>>,
    threads: &mut Vec<std::thread::JoinHandle<()>>,
) {
    let mounts = if mounts.is_empty() {
        discover_local_mounts()
    } else {
        mounts
    };
    for mount in mounts {
        let out = Arc::clone(out);
        let index = index.map(Arc::clone);
        threads.push(std::thread::spawn(move || run_scan(mount, out, index)));
    }
}

fn send(out: &Arc<Mutex<BufWriter<Stdout>>>, msg: &HelperMsg) -> Result<()> {
    let mut w = out.lock().unwrap();
    write_frame(&mut *w, msg)?;
    Ok(())
}

fn send_lossy(out: &Arc<Mutex<BufWriter<Stdout>>>, msg: &HelperMsg) {
    if let Err(e) = send(out, msg) {
        tracing::warn!("failed to send frame: {e}");
    }
}

/// Scan one mount through its filesystem-native lane, streaming batches.
fn run_scan(
    mount: MountInfo,
    out: Arc<Mutex<BufWriter<Stdout>>>,
    index: Option<Arc<RwLock<Index>>>,
) {
    send_lossy(
        &out,
        &HelperMsg::ScanBegin {
            mount: mount.clone(),
        },
    );

    let started = Instant::now();
    let mut batch: Vec<FileRecord> = Vec::with_capacity(RECORD_BATCH);
    let mut counts = (0u64, 0u64); // dirs, files
    let result = {
        let out = &out;
        let mut sink = |rec: FileRecord| {
            match rec.kind {
                neutra_core::FileKind::Dir => counts.0 += 1,
                _ => counts.1 += 1,
            }
            batch.push(rec);
            if batch.len() >= RECORD_BATCH {
                if let Some(index) = &index {
                    index.write().unwrap().extend(batch.iter().cloned());
                }
                send_lossy(out, &HelperMsg::Records(std::mem::take(&mut batch)));
            }
        };
        dispatch_lane(&mount, &mut sink)
    };

    match result {
        Ok(mut stats) => {
            if !batch.is_empty() {
                if let Some(index) = &index {
                    index.write().unwrap().extend(batch.iter().cloned());
                }
                send_lossy(&out, &HelperMsg::Records(std::mem::take(&mut batch)));
            }
            stats.wall_ms = started.elapsed().as_millis() as u64;
            send_lossy(&out, &HelperMsg::ScanDone { mount, stats });
        }
        Err(e) => {
            send_lossy(
                &out,
                &HelperMsg::ScanError {
                    mount,
                    error: format!("{e:#}"),
                },
            );
        }
    }
}

fn discover_local_mounts() -> Vec<MountInfo> {
    #[cfg(target_os = "linux")]
    {
        return neutra_core::mounts::system_mounts()
            .unwrap_or_default()
            .into_iter()
            .filter(|m| m.fs.is_indexable_local())
            .collect();
    }
    #[cfg(target_os = "macos")]
    {
        return vec![MountInfo {
            device: "/".into(),
            mountpoint: "/".into(),
            fs: FsKind::Unsupported("apfs".into()),
            source: neutra_core::MountSource::Local,
        }];
    }
    #[cfg(target_os = "windows")]
    {
        return (b'A'..=b'Z')
            .filter_map(|letter| {
                let root = format!("{}:\\", letter as char);
                std::path::Path::new(&root).exists().then(|| MountInfo {
                    device: format!("{}:", letter as char),
                    mountpoint: root.into(),
                    fs: FsKind::Ntfs,
                    source: neutra_core::MountSource::Local,
                })
            })
            .collect();
    }
    #[allow(unreachable_code)]
    Vec::new()
}

/// Route a mount to its native lane. Unsupported combinations are explicit
/// errors — never a silent fallback to walking.
fn dispatch_lane(mount: &MountInfo, sink: &mut dyn FnMut(FileRecord)) -> Result<ScanStats> {
    match &mount.fs {
        #[cfg(target_os = "linux")]
        FsKind::Btrfs => neutra_btrfs::scan(mount, sink),
        #[cfg(target_os = "linux")]
        FsKind::Ext4 => neutra_ext4::scan(mount, sink),
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        FsKind::Ntfs => neutra_ntfs::scan(mount, sink),
        #[cfg(target_os = "linux")]
        FsKind::Zfs => neutra_zfs::scan(mount, sink),
        #[cfg(target_os = "macos")]
        FsKind::Unsupported(_) | FsKind::Zfs | FsKind::Ext4 | FsKind::Btrfs | FsKind::Ntfs => {
            // On macOS the unit of indexing is the volume via Spotlight,
            // regardless of what fstype string the client sent.
            neutra_macos::scan(mount, sink)
        }
        FsKind::Network(_) => anyhow::bail!(
            "network mounts are indexed by provisioning a helper on the server, not scanned locally"
        ),
        other => anyhow::bail!(
            "no native metadata lane for filesystem '{}' on {} — refusing to walk",
            other.label(),
            std::env::consts::OS
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neutra_core::{FileKind, FsKind};

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
        }
    }

    #[test]
    fn durable_store_syncs_and_searches_delta() {
        let base_path =
            std::env::temp_dir().join(format!("neutra-helper-store-{}.nsx", std::process::id()));
        let mut delta_path = base_path.clone();
        delta_path.set_extension("delta");
        let _ = std::fs::remove_file(&base_path);
        let _ = std::fs::remove_file(&delta_path);
        CompactIndex::build(&[record("/old.txt", 1)], &base_path).unwrap();

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
        std::fs::remove_file(base_path).unwrap();
        std::fs::remove_file(delta_path).unwrap();
    }
}
