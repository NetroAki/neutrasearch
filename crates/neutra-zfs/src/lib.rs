//! ZFS lane.
//!
//! A version-matched native reader streams ZAP directory entries and SA
//! metadata from a consistent snapshot through read-only libzpool. Its C
//! boundary compiles against the pinned OpenZFS headers; Rust consumes a
//! bounded binary record stream, not private C layouts or debug output.
//!
//! This crate also provides a `zfs diff` parser and snapshot command builders
//! for updates. Initial indexing requires the native backend; there is no
//! directory enumeration fallback.

use anyhow::{bail, Result};
use neutra_core::{FileRecord, MountInfo, ScanStats};
use std::path::Path;

#[cfg(target_os = "linux")]
mod capability;
#[cfg(target_os = "linux")]
mod dataset;
mod diff;
#[cfg(target_os = "linux")]
mod native;
mod probe;
#[cfg(target_os = "linux")]
mod programs;
#[cfg(target_os = "linux")]
mod staging;
#[cfg(target_os = "linux")]
mod wire;
pub use diff::{parse_diff, DiffEntry};
pub use probe::{probe, reader_version, ZfsLaneProbe};

pub const SNAPSHOT_PREFIX: &str = "neutra-";

pub fn snapshot_name(dataset: &str, unix_seconds: u64) -> Result<String> {
    if dataset.is_empty() || dataset.contains('@') || dataset.chars().any(char::is_whitespace) {
        bail!("invalid ZFS dataset name");
    }
    Ok(format!("{dataset}@{SNAPSHOT_PREFIX}{unix_seconds}"))
}

pub fn snapshot_command(dataset: &str, unix_seconds: u64, recursive: bool) -> Result<Vec<String>> {
    let mut args = vec!["snapshot".to_string()];
    if recursive {
        args.push("-r".into());
    }
    args.push(snapshot_name(dataset, unix_seconds)?);
    Ok(args)
}

pub fn destroy_snapshot_command(snapshot: &str) -> Result<Vec<String>> {
    let Some((_, tag)) = snapshot.rsplit_once('@') else {
        bail!("not a snapshot name");
    };
    if !tag.starts_with(SNAPSHOT_PREFIX) {
        bail!("refusing to destroy a snapshot not owned by neutrasearch");
    }
    Ok(vec!["destroy".into(), snapshot.into()])
}

/// Initial ZFS scan through the native metadata backend only.
pub fn scan(mount: &MountInfo, sink: &mut dyn FnMut(FileRecord)) -> Result<ScanStats> {
    #[cfg(target_os = "linux")]
    {
        native::scan(mount, sink)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (mount, sink);
        bail!(
            "The native ZFS metadata reader requires Linux/OpenZFS; directory walking is disabled"
        )
    }
}

pub fn is_neutrasearch_snapshot(path: &Path) -> bool {
    path.to_string_lossy()
        .rsplit_once('@')
        .is_some_and(|(_, tag)| tag.starts_with(SNAPSHOT_PREFIX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_diff_with_types_and_spaces() {
        let input = "+\tF\t/tank/a file.txt\n-\t/old\nM\tF\t/tank/m\nR\tF\t/tank/old name\t/tank/new name\n";
        assert_eq!(
            parse_diff(input).unwrap(),
            vec![
                DiffEntry::Created("/tank/a file.txt".into()),
                DiffEntry::Removed("/old".into()),
                DiffEntry::Modified("/tank/m".into()),
                DiffEntry::Renamed {
                    from: "/tank/old name".into(),
                    to: "/tank/new name".into()
                },
            ]
        );
    }

    #[test]
    fn snapshot_commands_are_bounded() {
        assert_eq!(
            snapshot_command("tank/data", 42, true).unwrap(),
            vec!["snapshot", "-r", "tank/data@neutra-42"]
        );
        assert!(destroy_snapshot_command("tank/data@manual").is_err());
        assert_eq!(
            destroy_snapshot_command("tank/data@neutra-42").unwrap(),
            vec!["destroy", "tank/data@neutra-42"]
        );
    }
}
