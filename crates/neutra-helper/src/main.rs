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

#[cfg(target_os = "linux")]
mod watch_linux;

use anyhow::{Context, Result};
use neutra_core::mounts::{FsKind, MountInfo};
use neutra_core::proto::{
    read_frame, write_frame, ClientMsg, HelperMsg, HELPER_BUILD, PROTO_VERSION,
};
use neutra_core::{
    CompactIndex, DeltaChange, DeltaIndex, FileRecord, Index, Query, ScanStats, SearchHit,
    SearchStats, DELTA_HEADER_BYTES,
};
use std::collections::{HashMap, HashSet};
use std::io::{BufWriter, Read, Stdout, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

const RECORD_BATCH: usize = 4096;

struct DurableStore {
    path: std::path::PathBuf,
    base: Option<CompactIndex>,
    delta: DeltaIndex,
}

struct CompactionResult {
    records: u64,
    bytes: u64,
}

struct ApplyResult {
    changes: u32,
    wal_bytes: u64,
    compacted: Option<CompactionResult>,
}

impl DurableStore {
    fn open(path: &std::path::Path) -> Result<Self> {
        Self::open_inner(path, None)
    }

    #[cfg(test)]
    fn open_with_threshold(path: &std::path::Path, compact_at: u64) -> Result<Self> {
        Self::open_inner(path, Some(compact_at))
    }

    fn open_inner(path: &std::path::Path, compact_at: Option<u64>) -> Result<Self> {
        let path = path.to_path_buf();
        let mut delta_path = path.clone();
        delta_path.set_extension("delta");
        let (base, delta) = open_durable_pair(&path, &delta_path, compact_at)?;
        Ok(Self {
            path,
            base: Some(base),
            delta,
        })
    }

    fn search(&self, query: &Query) -> Result<(Vec<SearchHit>, SearchStats)> {
        let base = self
            .base
            .as_ref()
            .context("compact base is unavailable after a failed replacement")?;
        Ok(base.search_with_delta(query, &self.delta)?)
    }

    fn apply(&mut self, changes: Vec<DeltaChange>) -> Result<(u32, u64, bool)> {
        let count = u32::try_from(changes.len()).context("delta batch is too large")?;
        for change in changes {
            self.delta.apply(change)?;
        }
        self.delta.sync()?;
        Ok((count, self.delta.wal_bytes(), self.delta.needs_compaction()))
    }

    fn apply_bounded(&mut self, changes: Vec<DeltaChange>) -> Result<ApplyResult> {
        let mut compacted = None;
        if self.delta.needs_compaction() {
            compacted = Some(self.compact()?);
        }
        let (changes, _, needs_compaction) = self.apply(changes)?;
        if needs_compaction {
            compacted = Some(self.compact()?);
        }
        Ok(ApplyResult {
            changes,
            wal_bytes: self.delta.wal_bytes(),
            compacted,
        })
    }

    /// Merge base+delta into a replacement base and reset the WAL. The caller
    /// holds the store write lock, so searches wait until the pair is coherent.
    fn compact(&mut self) -> Result<CompactionResult> {
        let base = self
            .base
            .as_ref()
            .context("compact base is unavailable after a failed replacement")?;
        let mut records = base.records().context("read base records for compaction")?;
        let removed = self
            .delta
            .removed()
            .map(|path| path.as_ref())
            .collect::<HashSet<&str>>();
        let upserts = self
            .delta
            .upserts()
            .map(|record| (record.path.as_ref(), record))
            .collect::<HashMap<&str, &FileRecord>>();
        records.retain(|record| {
            !removed.contains(record.path.as_ref()) && !upserts.contains_key(record.path.as_ref())
        });
        records.extend(upserts.values().map(|record| (*record).clone()));
        drop(upserts);
        drop(removed);

        self.delta.sync().context("sync delta before compaction")?;
        let staged = compaction_stage(&self.path);
        let marker = compaction_marker(&self.path);
        let built = CompactIndex::build(&records, &staged)
            .context("build staged replacement compact base")?;
        write_compaction_marker(&marker, built.generation)?;
        self.delta
            .reset(built.generation)
            .context("reset delta for replacement base")?;
        // Windows does not permit replacing a file while our old mmap is live.
        drop(self.base.take());
        CompactIndex::publish(&staged, &self.path).context("publish replacement compact base")?;
        let base = CompactIndex::open(&self.path).context("open replacement compact base")?;
        if base.generation() != built.generation {
            anyhow::bail!("published compact base generation changed unexpectedly");
        }
        self.base = Some(base);
        remove_compaction_marker(&marker)?;
        let _ = std::fs::remove_file(&staged);
        Ok(CompactionResult {
            records: built.records,
            bytes: built.bytes,
        })
    }
}

fn open_durable_pair(
    base_path: &std::path::Path,
    delta_path: &std::path::Path,
    compact_at: Option<u64>,
) -> Result<(CompactIndex, DeltaIndex)> {
    let marker = compaction_marker(base_path);
    if !marker.is_file() {
        let base = CompactIndex::open(base_path)
            .with_context(|| format!("open compact index {}", base_path.display()))?;
        let staged_path = compaction_stage(base_path);
        let short_wal_with_stage = staged_path.is_file()
            && std::fs::metadata(delta_path)
                .is_ok_and(|metadata| metadata.len() < DELTA_HEADER_BYTES);
        let current_delta = if short_wal_with_stage {
            Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "short WAL beside an unmarked compaction stage",
            ))
        } else {
            open_delta_writer(delta_path, base.generation(), compact_at)
        };
        let pair = match current_delta {
            Ok(delta) => (base, delta),
            Err(error) if recoverable_delta_error(&error) && staged_path.is_file() => {
                let staged =
                    CompactIndex::open(&staged_path).context("open unmarked compaction stage")?;
                let generation = staged.generation();
                let delta = match open_delta_writer(delta_path, generation, compact_at) {
                    Ok(delta) => delta,
                    Err(error) if recoverable_delta_error(&error) => {
                        replace_empty_delta(delta_path, generation, compact_at)
                            .context("replace torn WAL matching unmarked compaction stage")?
                    }
                    Err(error) => {
                        return Err(error).context("open WAL matching unmarked compaction stage");
                    }
                };
                drop(staged);
                drop(base);
                CompactIndex::publish(&staged_path, base_path)
                    .context("recover marker-lost base publication")?;
                let base =
                    CompactIndex::open(base_path).context("open marker-lost recovered base")?;
                if base.generation() != generation {
                    anyhow::bail!("marker-lost recovered base generation changed unexpectedly");
                }
                (base, delta)
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("open delta index {}", delta_path.display()));
            }
        };
        for stale in [staged_path, compaction_marker_temp(base_path)] {
            let _ = std::fs::remove_file(stale);
        }
        return Ok(pair);
    }

    let expected_generation = read_compaction_marker(&marker)?;
    let staged_path = compaction_stage(base_path);
    let base = CompactIndex::open(base_path)
        .with_context(|| format!("open compact index {}", base_path.display()))?;
    if base.generation() == expected_generation {
        let delta = match open_delta_writer(delta_path, expected_generation, compact_at) {
            Ok(delta) => delta,
            Err(error) if recoverable_delta_error(&error) => {
                replace_empty_delta(delta_path, expected_generation, compact_at)
                    .context("replace torn WAL after completed compaction")?
            }
            Err(error) => return Err(error).context("open delta after completed compaction"),
        };
        if staged_path.is_file() {
            let _ = std::fs::remove_file(&staged_path);
        }
        remove_compaction_marker(&marker)?;
        return Ok((base, delta));
    }

    let staged = CompactIndex::open(&staged_path)
        .with_context(|| format!("open compaction stage {}", staged_path.display()))?;
    if staged.generation() != expected_generation {
        anyhow::bail!("compaction marker and staged base generations differ");
    }
    drop(staged);

    let wal_needs_direct_replacement = match std::fs::metadata(delta_path) {
        Ok(metadata) => metadata.len() < DELTA_HEADER_BYTES,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(error) => return Err(error).context("inspect WAL during compaction recovery"),
    };
    let mut delta = if wal_needs_direct_replacement {
        replace_empty_delta(delta_path, expected_generation, compact_at)
            .context("replace short compaction WAL")?
    } else {
        match open_delta_writer(delta_path, base.generation(), compact_at) {
            Ok(mut delta) => {
                delta
                    .reset(expected_generation)
                    .context("finish compaction WAL reset")?;
                delta
            }
            Err(error) if recoverable_delta_error(&error) => {
                match open_delta_writer(delta_path, expected_generation, compact_at) {
                    Ok(delta) => delta,
                    Err(error) if recoverable_delta_error(&error) => {
                        replace_empty_delta(delta_path, expected_generation, compact_at)
                            .context("replace torn compaction WAL")?
                    }
                    Err(error) => {
                        return Err(error).context("reopen already-reset compaction WAL");
                    }
                }
            }
            Err(error) => {
                return Err(error).context("acquire delta writer during compaction recovery");
            }
        }
    };
    delta.sync()?;
    drop(base);
    CompactIndex::publish(&staged_path, base_path).context("finish staged base publication")?;
    let base = CompactIndex::open(base_path).context("open recovered compact base")?;
    if base.generation() != expected_generation {
        anyhow::bail!("recovered compact base generation changed unexpectedly");
    }
    remove_compaction_marker(&marker)?;
    let _ = std::fs::remove_file(&staged_path);
    Ok((base, delta))
}

fn open_delta_writer(
    path: &std::path::Path,
    generation: u64,
    compact_at: Option<u64>,
) -> std::io::Result<DeltaIndex> {
    match compact_at {
        Some(threshold) => DeltaIndex::open_with_threshold(path, generation, threshold),
        None => DeltaIndex::open(path, generation),
    }
}

fn replace_empty_delta(
    path: &std::path::Path,
    generation: u64,
    compact_at: Option<u64>,
) -> std::io::Result<DeltaIndex> {
    match compact_at {
        Some(threshold) => DeltaIndex::replace_empty_with_threshold(path, generation, threshold),
        None => DeltaIndex::replace_empty(path, generation),
    }
}

fn recoverable_delta_error(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::InvalidData | std::io::ErrorKind::UnexpectedEof
    )
}

fn compaction_stage(base: &std::path::Path) -> std::path::PathBuf {
    append_suffix(base, ".compact")
}

fn compaction_marker(base: &std::path::Path) -> std::path::PathBuf {
    append_suffix(base, ".compacting")
}

fn compaction_marker_temp(base: &std::path::Path) -> std::path::PathBuf {
    append_suffix(base, ".compacting.new")
}

fn append_suffix(path: &std::path::Path, suffix: &str) -> std::path::PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    value.into()
}

fn write_compaction_marker(path: &std::path::Path, generation: u64) -> Result<()> {
    if path.exists() {
        anyhow::bail!("compaction marker already exists: {}", path.display());
    }
    let temporary = append_suffix(path, ".new");
    let _ = std::fs::remove_file(&temporary);
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .with_context(|| format!("create compaction marker {}", temporary.display()))?;
    file.write_all(&generation.to_le_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temporary, path).with_context(|| {
        format!(
            "publish compaction marker {} -> {}",
            temporary.display(),
            path.display()
        )
    })?;
    sync_parent(path)
}

fn read_compaction_marker(path: &std::path::Path) -> Result<u64> {
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("open compaction marker {}", path.display()))?;
    if file.metadata()?.len() != 8 {
        anyhow::bail!("invalid compaction marker length");
    }
    let mut generation = [0u8; 8];
    file.read_exact(&mut generation)?;
    let generation = u64::from_le_bytes(generation);
    if generation == 0 {
        anyhow::bail!("invalid zero compaction generation");
    }
    Ok(generation)
}

fn remove_compaction_marker(path: &std::path::Path) -> Result<()> {
    std::fs::remove_file(path)
        .with_context(|| format!("remove compaction marker {}", path.display()))?;
    let _ = std::fs::remove_file(append_suffix(path, ".new"));
    sync_parent(path)
}

#[cfg(unix)]
fn sync_parent(path: &std::path::Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn sync_parent(_path: &std::path::Path) -> Result<()> {
    Ok(())
}

fn main() -> Result<()> {
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
        std::env::var_os("NEUTRASEARCH_SERVE_INDEX")
            .or_else(|| std::env::var_os("NEUTRA_SERVE_INDEX"))
            .map(std::path::PathBuf::from)
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
        Some("--scan-summary") => {
            let target = std::env::args()
                .nth(2)
                .context("use: neutrasearch index MOUNT --output INDEX.nsx")?;
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
            {
                let _ = target;
                anyhow::bail!("--scan-summary mount discovery is currently available on Linux");
            }
        }
        Some("--build-index") => {
            let target = std::env::args()
                .nth(2)
                .context("use: neutrasearch index MOUNT --output INDEX.nsx")?;
            let output = std::path::PathBuf::from(
                std::env::args()
                    .nth(3)
                    .context("use: neutrasearch index MOUNT --output INDEX.nsx")?,
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
            {
                let _ = (target, output);
                anyhow::bail!("--build-index mount discovery is currently available on Linux");
            }
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
        .as_ref()
        .map(|path| DurableStore::open(path).map(|store| Arc::new(RwLock::new(store))))
        .transpose()?;
    let stale = Arc::new(AtomicBool::new(false));
    #[cfg(target_os = "linux")]
    if let Some((mountpoint, source)) = watch_mount {
        let mount = neutra_core::mounts::system_mounts()?
            .into_iter()
            .find(|mount| mount.mountpoint == mountpoint)
            .with_context(|| format!("no supported mount at {}", mountpoint.display()))?;
        let base_path = serve_index.as_ref().expect("watch mode has an index");
        let watcher =
            watch_linux::FanotifyWatcher::open(mount, source, watch_exclusions(base_path))?;
        start_native_watch(
            watcher,
            Arc::clone(durable.as_ref().expect("watch mode has a durable store")),
            Arc::clone(&stale),
        );
    }
    #[cfg(not(target_os = "linux"))]
    if watch_mount.is_some() {
        anyhow::bail!(
            "native watch mode is not implemented on {}",
            std::env::consts::OS
        );
    }
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
                if stale.load(Ordering::Acquire) {
                    send(
                        &out,
                        &HelperMsg::Error(
                            "index is stale; run a full native reindex before searching".into(),
                        ),
                    )?;
                    continue;
                }
                let (hits, stats) = if let Some(store) = &durable {
                    let store = store.read().unwrap();
                    if stale.load(Ordering::Acquire) {
                        send(
                            &out,
                            &HelperMsg::Error(
                                "index became stale during the query; rebuild and restart the service"
                                    .into(),
                            ),
                        )?;
                        continue;
                    }
                    store.search(&query)?
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
                if stale.load(Ordering::Acquire) {
                    send(
                        &out,
                        &HelperMsg::Error(
                            "index is stale; run a full native reindex before applying changes"
                                .into(),
                        ),
                    )?;
                    continue;
                }
                let Some(store) = &durable else {
                    send(
                        &out,
                        &HelperMsg::Error(
                            "ApplyDelta requires 'neutrasearch serve --index INDEX.nsx'".into(),
                        ),
                    )?;
                    continue;
                };
                let mut store = store.write().unwrap();
                if stale.load(Ordering::Acquire) {
                    send(
                        &out,
                        &HelperMsg::Error(
                            "index became stale before the update; rebuild and restart the service"
                                .into(),
                        ),
                    )?;
                    continue;
                }
                match store.apply_bounded(changes) {
                    Ok(applied) => {
                        if let Some(compacted) = applied.compacted {
                            tracing::info!(
                                records = compacted.records,
                                bytes = compacted.bytes,
                                "compacted delta into replacement base"
                            );
                        }
                        send(
                            &out,
                            &HelperMsg::DeltaApplied {
                                changes: applied.changes,
                                wal_bytes: applied.wal_bytes,
                                needs_compaction: false,
                            },
                        )?;
                    }
                    Err(error) => {
                        stale.store(true, Ordering::Release);
                        send(
                            &out,
                            &HelperMsg::Error(format!(
                                "delta commit or compaction failed; index disabled until rebuild: {error:#}"
                            )),
                        )?;
                    }
                }
            }
        }
    }

    for t in scan_threads {
        let _ = t.join();
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn watch_exclusions(base: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut delta = base.to_path_buf();
    delta.set_extension("delta");
    let mut lock = delta.as_os_str().to_os_string();
    lock.push(".lock");
    let temporary = append_suffix(base, ".new");
    let staged = compaction_stage(base);
    let staged_temporary = append_suffix(&staged, ".new");
    let marker = compaction_marker(base);
    let marker_temporary = compaction_marker_temp(base);
    vec![
        base.to_path_buf(),
        delta,
        lock.into(),
        temporary,
        staged,
        staged_temporary,
        marker,
        marker_temporary,
    ]
}

#[cfg(target_os = "linux")]
fn start_native_watch(
    mut watcher: watch_linux::FanotifyWatcher,
    store: Arc<RwLock<DurableStore>>,
    stale: Arc<AtomicBool>,
) {
    std::thread::spawn(move || loop {
        match watcher.read_batch() {
            Ok(watch_linux::WatchBatch::Changes(changes)) if changes.is_empty() => {}
            Ok(watch_linux::WatchBatch::Changes(changes)) => {
                let applied = match store.write() {
                    Ok(mut store) => match store.apply_bounded(changes) {
                        Ok(applied) => Ok(applied),
                        Err(error) => {
                            // Publish the failure while the write lock is still
                            // held. Searches recheck stale after acquiring their
                            // read lock, so non-durable memory is never served.
                            stale.store(true, Ordering::Release);
                            Err(error)
                        }
                    },
                    Err(_) => {
                        stale.store(true, Ordering::Release);
                        Err(anyhow::anyhow!("durable store lock poisoned"))
                    }
                };
                match applied {
                    Ok(applied) => {
                        if let Some(compacted) = applied.compacted {
                            tracing::info!(
                                records = compacted.records,
                                bytes = compacted.bytes,
                                "compacted delta into replacement base"
                            );
                        }
                        tracing::debug!(
                            changes = applied.changes,
                            wal_bytes = applied.wal_bytes,
                            "native watch batch committed"
                        );
                    }
                    Err(error) => {
                        stale.store(true, Ordering::Release);
                        tracing::error!("native watch stopped: {error:#}");
                        break;
                    }
                }
            }
            Ok(watch_linux::WatchBatch::RescanRequired(reason)) => {
                stale.store(true, Ordering::Release);
                tracing::error!(reason, "native watch requires a full native reindex");
                break;
            }
            Err(error) => {
                stale.store(true, Ordering::Release);
                tracing::error!("native watch stopped: {error:#}");
                break;
            }
        }
    });
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

    #[test]
    fn durable_store_syncs_and_searches_delta() {
        let (base_path, delta_path) = store_paths("store");
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
        remove_store(&base_path, &delta_path);
    }

    #[test]
    fn ignores_unpublished_partial_marker_and_staged_base() {
        let (base_path, delta_path) = store_paths("partial-marker");
        CompactIndex::build(&[record("/old.txt", 1)], &base_path).unwrap();
        drop(DurableStore::open(&base_path).unwrap());
        let staged_path = compaction_stage(&base_path);
        CompactIndex::build(&[record("/new.txt", 2)], &staged_path).unwrap();
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
        CompactIndex::build(&[record("/old.txt", 1)], &base_path).unwrap();
        let mut store = DurableStore::open(&base_path).unwrap();
        store
            .apply(vec![
                DeltaChange::Remove("/old.txt".into()),
                DeltaChange::Upsert(record("/new.txt", 2)),
            ])
            .unwrap();
        drop(store);

        let staged_path = compaction_stage(&base_path);
        let built = CompactIndex::build(&[record("/new.txt", 2)], &staged_path).unwrap();
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
        CompactIndex::build(&[record("/old.txt", 1)], &base_path).unwrap();
        let mut store = DurableStore::open(&base_path).unwrap();
        store
            .apply(vec![
                DeltaChange::Remove("/old.txt".into()),
                DeltaChange::Upsert(record("/new.txt", 2)),
            ])
            .unwrap();
        let staged_path = compaction_stage(&base_path);
        let built = CompactIndex::build(&[record("/new.txt", 2)], &staged_path).unwrap();
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
            CompactIndex::build(&[record("/old.txt", 1)], &base_path).unwrap();
            let mut store = DurableStore::open(&base_path).unwrap();
            store
                .apply(vec![
                    DeltaChange::Remove("/old.txt".into()),
                    DeltaChange::Upsert(record("/new.txt", 2)),
                ])
                .unwrap();
            let staged_path = compaction_stage(&base_path);
            let built = CompactIndex::build(&[record("/new.txt", 2)], &staged_path).unwrap();
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
            CompactIndex::build(&[record("/old.txt", 1)], &base_path).unwrap();
            let mut store = DurableStore::open(&base_path).unwrap();
            store
                .apply(vec![
                    DeltaChange::Remove("/old.txt".into()),
                    DeltaChange::Upsert(record("/new.txt", 2)),
                ])
                .unwrap();
            let staged_path = compaction_stage(&base_path);
            let built = CompactIndex::build(&[record("/new.txt", 2)], &staged_path).unwrap();
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
    fn durable_store_compacts_base_and_resets_delta_generation() {
        let (base_path, delta_path) = store_paths("compact");
        CompactIndex::build(&[record("/old.txt", 1)], &base_path).unwrap();
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
        assert_eq!(store.delta.change_count(), 0);
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
