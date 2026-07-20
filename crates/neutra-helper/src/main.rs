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
use neutra_core::{CompactIndex, FileRecord, Index, ScanStats};
use std::io::{BufWriter, Stdout};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

const RECORD_BATCH: usize = 4096;

fn main() -> Result<()> {
    // `--version`/`--build` are used by auto-provisioning to decide whether a
    // remote copy is stale. Keep them dependency-free and instant.
    let arg = std::env::args().nth(1);
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
                let (hits, stats) = index.read().unwrap().search(&query);
                send(
                    &out,
                    &HelperMsg::SearchResult {
                        hits: hits.into_iter().map(|hit| hit.record).collect(),
                        wall_us: stats.wall_us,
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
