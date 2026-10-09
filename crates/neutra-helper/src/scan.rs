//! Scan orchestration: trusted mount resolution, approved-root validation,
//! lane dispatch, record streaming, and the per-platform mount discovery
//! helpers. Split from `main` so the protocol loop stays separate from what
//! a scan does.

use crate::protocol::send_lossy;
use crate::store::{
    append_suffix, compaction_marker, compaction_marker_temp, compaction_stage, stale_marker,
};
use anyhow::{Context, Result};
use neutra_core::mounts::{FsKind, MountInfo};
use neutra_core::proto::HelperMsg;
use neutra_core::{DeltaChange, FileRecord, Index, Query, ScanStats};
pub(crate) const RECORD_BATCH: usize = 1024;
pub(crate) const MAX_SCAN_MOUNTS: usize = 32;
pub(crate) const MAX_QUERY_RESULTS: usize = 10_000;
pub(crate) const MAX_QUERY_TERMS: usize = 32;
pub(crate) const MAX_QUERY_TEXT_BYTES: usize = 32 * 1024;
pub(crate) const MAX_DELTA_CHANGES: usize = 65_536;
pub(crate) const MAX_INDEX_PATH_BYTES: usize = 32 * 1024;

use std::collections::HashSet;
use std::io::{BufWriter, Write};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

pub(crate) type ProtocolOutput = Arc<Mutex<BufWriter<Box<dyn Write + Send>>>>;

pub(crate) fn prepare_scan(
    requested: Vec<MountInfo>,
    roots: Vec<std::path::PathBuf>,
    idle: bool,
) -> Result<(Vec<MountInfo>, Vec<std::path::PathBuf>)> {
    let mounts = resolve_scan_mounts(requested, discover_local_mounts(), idle)?;
    let roots = validate_scan_roots(roots, &mounts)?;
    tracing::info!(
        target: "neutra_helper::protocol",
        mounts = mounts.len(),
        roots = roots.len(),
        "scan preparation complete"
    );
    Ok((mounts, roots))
}

pub(crate) fn resolve_scan_mounts(
    requested: Vec<MountInfo>,
    trusted: Vec<MountInfo>,
    idle: bool,
) -> Result<Vec<MountInfo>> {
    if !idle {
        anyhow::bail!("a native scan is already running");
    }
    if requested.len() > MAX_SCAN_MOUNTS {
        anyhow::bail!("scan request exceeds the {MAX_SCAN_MOUNTS}-mount limit");
    }
    if requested.is_empty() {
        return Ok(Vec::new());
    }
    let mut seen = HashSet::new();
    let mut resolved = Vec::with_capacity(requested.len());
    for request in requested {
        let mount = trusted
            .iter()
            .find(|mount| mount.mountpoint == request.mountpoint)
            .with_context(|| {
                format!(
                    "requested mount {} is not present in the trusted OS mount table",
                    request.mountpoint.display()
                )
            })?;
        let key = mount.mountpoint.to_string_lossy().into_owned();
        if seen.insert(key) {
            resolved.push(mount.clone());
        }
    }
    Ok(resolved)
}

pub(crate) fn validate_scan_roots(
    roots: Vec<std::path::PathBuf>,
    mounts: &[MountInfo],
) -> Result<Vec<std::path::PathBuf>> {
    if roots.len() > MAX_SCAN_MOUNTS {
        anyhow::bail!("scan roots exceed the {MAX_SCAN_MOUNTS}-root limit");
    }
    if mounts.is_empty() && roots.is_empty() {
        return Ok(Vec::new());
    }
    if roots.is_empty() {
        anyhow::bail!("scan requests must include at least one approved root");
    }
    let mut approved = Vec::<std::path::PathBuf>::with_capacity(roots.len());
    for root in roots {
        if !root.is_absolute() || !safe_absolute_path(&root.to_string_lossy()) {
            anyhow::bail!("scan roots must be absolute and normalized");
        }
        if !mounts
            .iter()
            .any(|mount| portable_path_in_root(&root.to_string_lossy(), &mount.mountpoint))
        {
            anyhow::bail!(
                "approved root {} is outside the requested native mounts",
                root.display()
            );
        }
        if !approved
            .iter()
            .any(|existing| same_portable_path(existing, &root))
        {
            approved.push(root);
        }
    }
    Ok(approved)
}

pub(crate) fn validate_query(query: &Query) -> Result<()> {
    if query.limit == 0 || query.limit > MAX_QUERY_RESULTS {
        anyhow::bail!("query limit must be between 1 and {MAX_QUERY_RESULTS}");
    }
    if query.terms.len() > MAX_QUERY_TERMS {
        anyhow::bail!("query exceeds the {MAX_QUERY_TERMS}-term limit");
    }
    if query.scope_roots.len() > MAX_SCAN_MOUNTS
        || query
            .scope_roots
            .iter()
            .any(|root| !std::path::Path::new(root).is_absolute())
    {
        anyhow::bail!("query scopes must be absolute and limited to {MAX_SCAN_MOUNTS} roots");
    }
    let text_bytes = query.terms.iter().map(String::len).sum::<usize>()
        + query.exts.iter().map(String::len).sum::<usize>()
        + query.scope_roots.iter().map(String::len).sum::<usize>()
        + query.under.as_ref().map_or(0, String::len);
    if text_bytes > MAX_QUERY_TEXT_BYTES {
        anyhow::bail!("query text exceeds the {MAX_QUERY_TEXT_BYTES}-byte limit");
    }
    Ok(())
}

pub(crate) fn validate_delta_changes(changes: &[DeltaChange]) -> Result<()> {
    if changes.len() > MAX_DELTA_CHANGES {
        anyhow::bail!("delta batch exceeds the {MAX_DELTA_CHANGES}-change limit");
    }
    for change in changes {
        let path = match change {
            DeltaChange::Upsert(record) => record.path.as_ref(),
            DeltaChange::Remove(path) => path.as_ref(),
        };
        if path.is_empty() || path.len() > MAX_INDEX_PATH_BYTES {
            anyhow::bail!("delta path length is outside the supported range");
        }
        if !safe_absolute_path(path) {
            anyhow::bail!("delta paths must be absolute and normalized");
        }
    }
    Ok(())
}

#[allow(clippy::ptr_arg)]
pub(crate) fn launch_scans(
    mounts: Vec<MountInfo>,
    roots: Vec<std::path::PathBuf>,
    out: &ProtocolOutput,
    index: Option<&Arc<RwLock<Index>>>,
    threads: &mut Vec<std::thread::JoinHandle<()>>,
) {
    #[cfg(target_os = "windows")]
    {
        let out = Arc::clone(out);
        let index = index.map(Arc::clone);
        let mount_count = mounts.len() as u32;
        let mut errors = 0u32;
        tracing::info!(
            target: "neutra_helper::protocol",
            mounts = mount_count,
            "native Windows scans running on service pipe thread"
        );
        for mount in mounts {
            if !run_scan(
                mount,
                &roots,
                Arc::clone(&out),
                index.as_ref().map(Arc::clone),
            ) {
                errors += 1;
            }
        }
        send_lossy(
            &out,
            &HelperMsg::ScanComplete {
                mounts: mount_count,
                errors,
            },
        );
        let _ = threads;
    }

    #[cfg(not(target_os = "windows"))]
    {
        tracing::info!(
            target: "neutra_helper::protocol",
            mounts = mounts.len(),
            "native scan workers launching"
        );
        let out = Arc::clone(out);
        let index = index.map(Arc::clone);
        threads.push(std::thread::spawn(move || {
            let mount_count = mounts.len() as u32;
            let mut errors = 0u32;
            for mount in mounts {
                if !run_scan(
                    mount,
                    &roots,
                    Arc::clone(&out),
                    index.as_ref().map(Arc::clone),
                ) {
                    errors += 1;
                }
            }
            send_lossy(
                &out,
                &HelperMsg::ScanComplete {
                    mounts: mount_count,
                    errors,
                },
            );
        }));
    }
}

/// Scan one mount through its filesystem-native lane, streaming batches.
pub(crate) fn run_scan(
    mount: MountInfo,
    roots: &[std::path::PathBuf],
    out: ProtocolOutput,
    index: Option<Arc<RwLock<Index>>>,
) -> bool {
    tracing::info!(target: "neutra_helper::protocol", "native scan worker started");
    send_lossy(
        &out,
        &HelperMsg::ScanBegin {
            mount: mount.clone(),
        },
    );
    tracing::info!(target: "neutra_helper::protocol", "native scan begin emitted");

    let started = Instant::now();
    let mountpoint = mount.mountpoint.clone();
    // Precompute the per-scan predicates once. The record path is already
    // portable ('/'-separated) from every native lane, so the hot loop only
    // does prefix checks — no per-record path normalization or joins.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    let root_prefixes = roots
        .iter()
        .map(|root| {
            neutra_core::paths::portable_root_prefix(&root.to_string_lossy()).to_ascii_lowercase()
        })
        .collect::<Vec<_>>();
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let root_prefixes = roots
        .iter()
        .map(|root| neutra_core::paths::portable_root_prefix(&root.to_string_lossy()))
        .collect::<Vec<_>>();
    let exclusion_prefixes = exclusion_prefixes(&mountpoint);
    let mut batch: Vec<FileRecord> = Vec::with_capacity(RECORD_BATCH);
    let mut counts = (0u64, 0u64); // dirs, files
    let result = {
        let out = &out;
        let mut sink = |rec: FileRecord| {
            let path = rec.path.as_ref();
            if exclusion_prefixes
                .iter()
                .any(|prefix| path_has_component_prefix(path, prefix))
                || !root_prefixes.iter().any(|root| record_in_root(path, root))
            {
                return;
            }
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
    tracing::info!(
        target: "neutra_helper::protocol",
        success = result.is_ok(),
        "native scan worker finished"
    );

    match result {
        Ok(mut stats) => {
            if !batch.is_empty() {
                if let Some(index) = &index {
                    index.write().unwrap().extend(batch.iter().cloned());
                }
                send_lossy(&out, &HelperMsg::Records(std::mem::take(&mut batch)));
            }
            stats.records = counts.0 + counts.1;
            stats.dirs = counts.0;
            stats.files = counts.1;
            stats.wall_ms = started.elapsed().as_millis() as u64;
            send_lossy(&out, &HelperMsg::ScanDone { mount, stats });
            true
        }
        Err(e) => {
            send_lossy(
                &out,
                &HelperMsg::ScanError {
                    mount,
                    error: format!("{e:#}"),
                },
            );
            false
        }
    }
}

/// Route a mount to its native lane. Unsupported combinations are explicit
/// errors — never a silent fallback to walking.
pub(crate) fn dispatch_lane(
    mount: &MountInfo,
    sink: &mut dyn FnMut(FileRecord),
) -> Result<ScanStats> {
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
        #[cfg(not(target_os = "macos"))]
        other => anyhow::bail!(
            "no native metadata lane for filesystem '{}' on {} — refusing to walk",
            other.label(),
            std::env::consts::OS
        ),
    }
}

pub(crate) fn portable_path_in_root(path: &str, root: &std::path::Path) -> bool {
    let case_sensitive = cfg!(not(any(target_os = "windows", target_os = "macos")));
    let normalize = |value: &str| {
        let value = neutra_core::paths::portable_root_prefix(value);
        if case_sensitive {
            value
        } else {
            value.to_ascii_lowercase()
        }
    };
    neutra_core::paths::path_in_portable_root(&normalize(path), &normalize(&root.to_string_lossy()))
}

/// Hot-loop containment for a record path against a precomputed root prefix.
/// Case-insensitive platforms fold the record path per record (as before);
/// case-sensitive platforms are allocation-free.
pub(crate) fn record_in_root(path: &str, root: &str) -> bool {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        let path = path.to_ascii_lowercase();
        path == root
            || (root.ends_with('/') && path.starts_with(&root))
            || (path.len() > root.len()
                && !root.ends_with('/')
                && path.starts_with(&root)
                && path.as_bytes()[root.len()] == b'/')
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        neutra_core::paths::path_in_portable_root(path, root)
    }
}

/// Path prefixes excluded from every scan (snapshots; kernel trees on `/`).
/// Computed once per scan.
pub(crate) fn exclusion_prefixes(mountpoint: &std::path::Path) -> Vec<String> {
    let mut prefixes = vec![format!(
        "{}/.snapshots",
        mountpoint.to_string_lossy().trim_end_matches('/')
    )];
    if mountpoint == std::path::Path::new("/") {
        prefixes.push("/proc".into());
        prefixes.push("/sys".into());
    }
    prefixes
}

pub(crate) fn path_has_component_prefix(path: &str, prefix: &str) -> bool {
    if path == prefix {
        return true;
    }
    path.len() > prefix.len() && path.starts_with(prefix) && path.as_bytes()[prefix.len()] == b'/'
}

pub(crate) fn same_portable_path(left: &std::path::Path, right: &std::path::Path) -> bool {
    let case_sensitive = cfg!(not(any(target_os = "windows", target_os = "macos")));
    let normalize = |path: &std::path::Path| {
        let value = path.to_string_lossy().replace('\\', "/");
        if case_sensitive {
            value
        } else {
            value.to_ascii_lowercase()
        }
    };
    normalize(left).trim_end_matches('/') == normalize(right).trim_end_matches('/')
}

pub(crate) fn find_local_mount(target: &str) -> Result<MountInfo> {
    discover_local_mounts()
        .into_iter()
        .find(|mount| same_mountpoint(&mount.mountpoint, std::path::Path::new(target)))
        .with_context(|| {
            format!(
                "no supported native filesystem is mounted at {target} on {}",
                std::env::consts::OS
            )
        })
}

#[cfg(target_os = "windows")]
pub(crate) fn same_mountpoint(left: &std::path::Path, right: &std::path::Path) -> bool {
    fn normalized(path: &std::path::Path) -> String {
        path.to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_ascii_lowercase()
    }
    normalized(left) == normalized(right)
}

pub(crate) fn discover_local_mounts() -> Vec<MountInfo> {
    #[cfg(target_os = "linux")]
    {
        return neutra_core::mounts::system_mounts()
            .unwrap_or_default()
            .into_iter()
            .filter(|mount| mount.fs.is_indexable_local())
            .collect();
    }
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("/sbin/mount").output();
        let mounts = output
            .ok()
            .filter(|output| output.status.success())
            .map(|output| parse_macos_mount_output(&String::from_utf8_lossy(&output.stdout)))
            .unwrap_or_default();
        if !mounts.is_empty() {
            return mounts;
        }
        return vec![macos_mount("/dev/root", "/", "apfs")];
    }
    #[cfg(target_os = "windows")]
    {
        return windows_local_mounts();
    }
    #[allow(unreachable_code)]
    Vec::new()
}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn macos_mount(device: &str, mountpoint: &str, filesystem: &str) -> MountInfo {
    MountInfo {
        device: device.into(),
        mountpoint: mountpoint.into(),
        // The macOS dispatch lane intentionally routes APFS/HFS volumes through
        // Spotlight/getattrlistbulk even though FsKind has no APFS variant.
        fs: FsKind::Unsupported(filesystem.to_ascii_lowercase()),
        source: neutra_core::MountSource::Local,
    }
}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn parse_macos_mount_output(output: &str) -> Vec<MountInfo> {
    output
        .lines()
        .filter_map(|line| {
            let (device, mounted) = line.split_once(" on ")?;
            let (mountpoint, options) = mounted.rsplit_once(" (")?;
            let filesystem = options.trim_end_matches(')').split(',').next()?.trim();
            if !matches!(filesystem, "apfs" | "hfs") || mountpoint.starts_with("/System/Volumes/") {
                return None;
            }
            Some(macos_mount(device, mountpoint, filesystem))
        })
        .collect()
}

#[cfg(any(target_os = "windows", test))]
pub(crate) fn run_bounded<T, F>(
    label: &'static str,
    timeout: std::time::Duration,
    operation: F,
) -> Option<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let _ = sender.send(operation());
    });
    match receiver.recv_timeout(timeout) {
        Ok(value) => Some(value),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            tracing::warn!(
                operation = label,
                ?timeout,
                "bounded operation timed out; worker left to finish"
            );
            None
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            tracing::warn!(
                operation = label,
                "bounded operation worker exited without a result"
            );
            None
        }
    }
}

#[cfg(target_os = "windows")]
fn windows_local_mounts() -> Vec<MountInfo> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;

    #[allow(non_snake_case)]
    #[link(name = "kernel32")]
    extern "system" {
        fn GetLogicalDriveStringsW(length: u32, buffer: *mut u16) -> u32;
        fn GetDriveTypeW(root: *const u16) -> u32;
        fn GetVolumeInformationW(
            root: *const u16,
            volume_name: *mut u16,
            volume_name_len: u32,
            serial: *mut u32,
            max_component_len: *mut u32,
            flags: *mut u32,
            filesystem_name: *mut u16,
            filesystem_name_len: u32,
        ) -> i32;
    }

    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_FIXED: u32 = 3;
    let Some(required) = run_bounded(
        "GetLogicalDriveStringsW(length)",
        std::time::Duration::from_millis(500),
        || unsafe { GetLogicalDriveStringsW(0, std::ptr::null_mut()) },
    ) else {
        return Vec::new();
    };
    if required == 0 {
        return Vec::new();
    }
    let capacity = required as usize + 1;
    let Some((written, buffer)) = run_bounded(
        "GetLogicalDriveStringsW(values)",
        std::time::Duration::from_millis(500),
        move || {
            let mut buffer = vec![0u16; capacity];
            let written =
                unsafe { GetLogicalDriveStringsW(buffer.len() as u32, buffer.as_mut_ptr()) };
            (written, buffer)
        },
    ) else {
        return Vec::new();
    };
    if written == 0 || written as usize >= buffer.len() {
        return Vec::new();
    }

    let mut mounts = Vec::new();
    let mut offset = 0usize;
    while offset < written as usize {
        let Some(length) = buffer[offset..]
            .iter()
            .position(|character| *character == 0)
        else {
            break;
        };
        if length == 0 {
            break;
        }
        let root = &buffer[offset..offset + length + 1];
        offset += length + 1;
        let drive_root = root.to_vec();
        let Some(drive_type) = run_bounded(
            "GetDriveTypeW",
            std::time::Duration::from_millis(500),
            move || unsafe { GetDriveTypeW(drive_root.as_ptr()) },
        ) else {
            tracing::warn!(drive = %String::from_utf16_lossy(&root[..length]), "skipping drive after drive-type timeout");
            continue;
        };
        if !matches!(drive_type, DRIVE_FIXED | DRIVE_REMOVABLE) {
            continue;
        }
        let volume_root = root.to_vec();
        let Some((ok, filesystem)) = run_bounded(
            "GetVolumeInformationW",
            std::time::Duration::from_millis(500),
            move || {
                let mut filesystem = [0u16; 32];
                let ok = unsafe {
                    GetVolumeInformationW(
                        volume_root.as_ptr(),
                        std::ptr::null_mut(),
                        0,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        filesystem.as_mut_ptr(),
                        filesystem.len() as u32,
                    )
                };
                (ok, filesystem)
            },
        ) else {
            tracing::warn!(drive = %String::from_utf16_lossy(&root[..length]), "skipping drive after volume metadata timeout");
            continue;
        };
        if ok == 0 {
            continue;
        }
        let filesystem_len = filesystem
            .iter()
            .position(|character| *character == 0)
            .unwrap_or(filesystem.len());
        let filesystem = String::from_utf16_lossy(&filesystem[..filesystem_len]);
        if !filesystem.eq_ignore_ascii_case("ntfs") {
            continue;
        }
        let mountpoint = std::path::PathBuf::from(OsString::from_wide(&root[..length]));
        let device = mountpoint
            .to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .to_owned();
        mounts.push(MountInfo {
            device,
            mountpoint,
            fs: FsKind::Ntfs,
            source: neutra_core::MountSource::Local,
        });
    }
    mounts
}

pub(crate) fn safe_absolute_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    let windows_absolute = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\')
        || path.starts_with("\\\\");
    !path.contains('\0')
        && (std::path::Path::new(path).is_absolute() || windows_absolute)
        && !path
            .split(['/', '\\'])
            .any(|component| matches!(component, "." | ".."))
}

pub(crate) fn reap_scan_threads(threads: &mut Vec<std::thread::JoinHandle<()>>) {
    let mut index = 0;
    while index < threads.len() {
        if threads[index].is_finished() {
            let thread = threads.swap_remove(index);
            if thread.join().is_err() {
                tracing::error!("native scan worker panicked");
            }
        } else {
            index += 1;
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn watch_exclusions(base: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut delta = base.to_path_buf();
    delta.set_extension("delta");
    let mut lock = delta.as_os_str().to_os_string();
    lock.push(".lock");
    let temporary = append_suffix(base, ".new");
    let staged = compaction_stage(base);
    let staged_temporary = append_suffix(&staged, ".new");
    let marker = compaction_marker(base);
    let marker_temporary = compaction_marker_temp(base);
    let stale = stale_marker(base);
    let stale_temporary = append_suffix(&stale, ".new");
    let overlay_paths = neutra_core::DeltaIndex::exclusion_roots(&delta);
    let mut paths = vec![
        base.to_path_buf(),
        append_suffix(&delta, ".checkpoint"),
        append_suffix(&delta, ".migrate"),
        base.with_extension("spill"),
        base.with_extension("spill.lock"),
        delta,
        lock.into(),
        temporary,
        staged,
        staged_temporary,
        marker,
        marker_temporary,
        stale,
        stale_temporary,
    ];
    paths.extend(overlay_paths);
    for path in [base.to_path_buf(), compaction_stage(base)] {
        for suffix in [
            ".browse",
            ".browse.new",
            ".browse.lock",
            ".browse.base",
            ".browse.base.new",
            ".browse-wal",
            ".browse-shm",
            ".dirs",
            ".dirs.new",
            ".tree",
            ".tree.new",
            ".rank",
            ".rank.new",
            ".sweep",
            ".sweep.new",
            ".sweep.tmp",
            ".watch-status",
            ".watch-status.new",
        ] {
            paths.push(append_suffix(&path, suffix));
        }
    }
    paths
}

#[cfg(not(target_os = "windows"))]
fn same_mountpoint(left: &std::path::Path, right: &std::path::Path) -> bool {
    left == right
}
