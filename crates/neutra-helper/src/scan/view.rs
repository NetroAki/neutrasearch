use super::{
    exclusion_prefixes, path_has_component_prefix, record_in_root, ProtocolOutput, RECORD_BATCH,
};
use crate::protocol::send_lossy;
use anyhow::Result;
use neutra_core::mounts::{FsKind, MountInfo};
use neutra_core::proto::HelperMsg;
use neutra_core::{FileRecord, Index, ScanStats};
use std::sync::{Arc, RwLock};
use std::time::Instant;

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

    #[cfg(target_os = "linux")]
    if mount.fs == FsKind::Btrfs {
        let cursor = (|| -> Result<neutra_core::NativeCursor> {
            let subvolume = neutra_btrfs::Subvolume::open(&mount.mountpoint)?;
            Ok(neutra_core::NativeCursor {
                mount: mount.mountpoint.to_string_lossy().into_owned(),
                tree: subvolume.tree_id(),
                since: subvolume.current_generation()?,
            })
        })();
        match cursor {
            Ok(cursor) => send_lossy(&out, &HelperMsg::ScanCursor { cursor }),
            Err(error) => {
                send_lossy(
                    &out,
                    &HelperMsg::ScanError {
                        mount,
                        error: format!("cannot capture Btrfs ingest cursor: {error:#}"),
                    },
                );
                return false;
            }
        }
    }

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
