use super::*;
use crate::protocol::run_protocol_with_auth;
use crate::scan::{
    parse_macos_mount_output, portable_path_in_root, resolve_scan_mounts, run_bounded,
    validate_delta_changes, validate_query, validate_scan_roots, MAX_QUERY_RESULTS,
};
use crate::store::{append_suffix, compaction_marker, compaction_marker_temp, compaction_stage};
use neutra_core::mounts::MountInfo;
use neutra_core::proto::{read_frame, write_frame, ClientMsg, HelperMsg, PROTO_VERSION};
use neutra_core::{DeltaChange, FileKind, FileRecord, FsKind, Query};
use std::sync::atomic::Ordering;
use std::sync::Arc;

pub(crate) fn record(path: &str, size: u64) -> FileRecord {
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

pub(crate) fn store_paths(label: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let base = std::env::temp_dir().join(format!(
        "neutrasearch-helper-{label}-{}.nsx",
        std::process::id()
    ));
    let mut delta = base.clone();
    delta.set_extension("delta");
    remove_store(&base, &delta);
    (base, delta)
}

pub(crate) fn build_test_base(
    records: &[FileRecord],
    path: &std::path::Path,
) -> anyhow::Result<neutra_core::CompactBuildStats> {
    let mut spill = neutra_core::SpillAccumulator::begin(path)?;
    spill.push_batch(records.to_vec())?;
    Ok(CompactIndex::rebuild_streamed(spill.finish()?, path)?)
}

pub(crate) fn remove_store(base: &std::path::Path, delta: &std::path::Path) {
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

mod authentication;
mod compaction;
mod directories;
mod protocol;
mod recovery;
mod scan;
mod store;
