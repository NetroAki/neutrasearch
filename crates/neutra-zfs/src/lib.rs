//! ZFS lane.
//!
//! OpenZFS exposes no stable userspace metadata-enumeration API: the intended
//! production path is snapshot + DMU/ZAP enumeration through libzpool, which
//! only exists inside an OpenZFS build tree (`zfs-libzpool` feature; the
//! default build refuses to pretend an unverified ABI is safe). `zdb` output
//! is deliberately never parsed: it is a debugging interface, not a
//! production API.
//!
//! This crate also provides a `zfs diff` parser and snapshot command builders
//! for updates. Initial indexing requires the native backend; there is no
//! directory enumeration fallback.

use anyhow::{bail, Result};
use neutra_core::{FileRecord, MountInfo, ScanStats};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffEntry {
    Created(PathBuf),
    Removed(PathBuf),
    Modified(PathBuf),
    Renamed { from: PathBuf, to: PathBuf },
}

/// Parse `zfs diff -FH old@snap new@snap`. `-H` makes it tab separated and
/// `-F` adds type information after the change marker; fields after paths are
/// intentionally ignored so this remains compatible across OpenZFS releases.
pub fn parse_diff(input: &str) -> Result<Vec<DiffEntry>> {
    let mut out = Vec::new();
    for (line_no, line) in input.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        let marker = cols.first().copied().unwrap_or("").trim();
        let path_at = |i: usize| -> Result<PathBuf> {
            cols.get(i)
                .map(|s| PathBuf::from(*s))
                .ok_or_else(|| anyhow::anyhow!("zfs diff line {} missing path", line_no + 1))
        };
        // Common output is marker, path; with -F it can be marker, type, path.
        let first_path = if cols.len() >= 3 { 2 } else { 1 };
        let entry = match marker {
            "+" => DiffEntry::Created(path_at(first_path)?),
            "-" => DiffEntry::Removed(path_at(first_path)?),
            "M" => DiffEntry::Modified(path_at(first_path)?),
            "R" => {
                let from = path_at(first_path)?;
                let to = path_at(first_path + 1)?;
                DiffEntry::Renamed { from, to }
            }
            other => bail!("unknown zfs diff marker {other:?} on line {}", line_no + 1),
        };
        out.push(entry);
    }
    Ok(out)
}

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
    #[cfg(feature = "zfs-libzpool")]
    {
        return libzpool_backend::scan(mount, sink);
    }
    #[cfg(not(feature = "zfs-libzpool"))]
    {
        let _ = (mount, sink);
        bail!(
            "ZFS native metadata indexing is unavailable in this build; directory walking is disabled"
        )
    }
}

/// Which ZFS indexing lanes are usable on this machine. Reported by
/// `neutrasearch-helper --zfs-probe` so operators can see the decision
/// without reading source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZfsLaneProbe {
    /// First line of `zfs version`, when the CLI is installed.
    pub zfs_cli_version: Option<String>,
    /// A libzpool shared object found on disk (the native ZAP lane's link
    /// target; the feature build must still link and verify against it).
    pub libzpool_soname: Option<&'static str>,
}

pub fn probe() -> ZfsLaneProbe {
    let zfs_cli_version = std::process::Command::new("zfs")
        .arg("version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .unwrap_or_default()
                .trim()
                .to_owned()
        })
        .filter(|line| !line.is_empty());
    let libzpool_soname = ["libzpool.so.6", "libzpool.so.4", "libzpool.so.2"]
        .into_iter()
        .find(|soname| {
            ["/usr/lib", "/usr/lib64", "/lib", "/usr/local/lib"]
                .iter()
                .any(|dir| std::path::Path::new(dir).join(soname).is_file())
        });
    ZfsLaneProbe {
        zfs_cli_version,
        libzpool_soname,
    }
}

#[cfg(feature = "zfs-libzpool")]
mod libzpool_backend {
    use super::*;

    // The stable public Rust ABI does not exist. This module intentionally
    // fails at runtime until OPENZFS_SRC-specific bindings are generated.
    // Keeping it feature-gated prevents pretending an unverified ABI is safe.
    pub fn scan(_mount: &MountInfo, _sink: &mut dyn FnMut(FileRecord)) -> Result<ScanStats> {
        bail!("zfs-libzpool feature enabled, but generated OpenZFS-version bindings are not installed; set OPENZFS_SRC and generate bindings for that exact release")
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
