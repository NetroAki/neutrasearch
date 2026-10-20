//! Scan orchestration and compatibility exports for scan helpers.

use anyhow::{Context, Result};
use neutra_core::mounts::MountInfo;
use std::collections::HashSet;

pub(crate) const RECORD_BATCH: usize = 1024;
pub(crate) const MAX_SCAN_MOUNTS: usize = 32;
pub(crate) const MAX_QUERY_RESULTS: usize = 10_000;
pub(crate) const MAX_QUERY_TERMS: usize = 32;
pub(crate) const MAX_QUERY_TEXT_BYTES: usize = 32 * 1024;
pub(crate) const MAX_DELTA_CHANGES: usize = 65_536;
pub(crate) const MAX_INDEX_PATH_BYTES: usize = 32 * 1024;

mod adapters;
mod domain;
mod state;
mod view;

#[cfg(test)]
pub(crate) use adapters::parse_macos_mount_output;
#[cfg(test)]
pub(crate) use adapters::run_bounded;
pub(crate) use adapters::{discover_local_mounts, find_local_mount};
pub(crate) use domain::{
    exclusion_prefixes, path_has_component_prefix, portable_path_in_root, record_in_root,
    safe_absolute_path, same_portable_path, validate_delta_changes, validate_query,
};
#[cfg(target_os = "linux")]
pub(crate) use state::is_watch_excluded;
pub(crate) use state::reap_scan_threads;
#[cfg(target_os = "linux")]
pub(crate) use state::watch_exclusions;
pub(crate) use view::{dispatch_lane, launch_scans};

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

pub(crate) type ProtocolOutput =
    std::sync::Arc<std::sync::Mutex<std::io::BufWriter<Box<dyn std::io::Write + Send>>>>;
