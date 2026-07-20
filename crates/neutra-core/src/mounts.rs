//! Filesystem/mount discovery.
//!
//! Linux: parsed from /proc/self/mountinfo. Windows: the helper enumerates
//! volumes natively (see neutra-ntfs windows backend); this module's
//! `mountinfo` parser is unix-only.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;

/// Filesystem families neutrasearch has (or plans) a native metadata lane for.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FsKind {
    Btrfs,
    Ext4,
    Ntfs,
    Zfs,
    /// Network mount (nfs, cifs/smb, sshfs, ...). Never indexed client-side;
    /// handled by auto-provisioning a remote helper.
    Network(String),
    /// Anything else: not indexable without walking, therefore unsupported.
    Unsupported(String),
}

impl FsKind {
    pub fn from_fstype(fstype: &str) -> FsKind {
        match fstype {
            "btrfs" => FsKind::Btrfs,
            "ext4" | "ext3" | "ext2" => FsKind::Ext4,
            "ntfs" | "ntfs3" | "ntfs-3g" | "fuseblk" => FsKind::Ntfs,
            "zfs" => FsKind::Zfs,
            n @ ("nfs" | "nfs4" | "cifs" | "smb3" | "smbfs" | "sshfs" | "fuse.sshfs" | "9p"
            | "virtiofs" | "glusterfs" | "ceph" | "fuse.ceph") => FsKind::Network(n.to_string()),
            other => FsKind::Unsupported(other.to_string()),
        }
    }

    pub fn label(&self) -> String {
        match self {
            FsKind::Btrfs => "btrfs".into(),
            FsKind::Ext4 => "ext4".into(),
            FsKind::Ntfs => "ntfs".into(),
            FsKind::Zfs => "zfs".into(),
            FsKind::Network(n) => format!("net:{n}"),
            FsKind::Unsupported(n) => n.clone(),
        }
    }

    pub fn is_indexable_local(&self) -> bool {
        matches!(
            self,
            FsKind::Btrfs | FsKind::Ext4 | FsKind::Ntfs | FsKind::Zfs
        )
    }
}

impl fmt::Display for FsKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label())
    }
}

/// Where a mount's index comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MountSource {
    /// Local privileged scan of a block device / local filesystem.
    Local,
    /// Remote neutrasearch-helper reached over SSH/TCP because this is a network mount.
    Remote { host: String },
}

/// One mounted filesystem worth indexing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountInfo {
    /// Block device or network spec as shown by the kernel
    /// ("/dev/sda2", "server:/export", "//win/share", ...).
    pub device: String,
    pub mountpoint: PathBuf,
    pub fs: FsKind,
    pub source: MountSource,
}

impl MountInfo {
    /// Extract server host for network mounts ("server:/export" -> "server",
    /// "//win/share" -> "win", "user@host:/path" -> "host").
    pub fn network_host(&self) -> Option<String> {
        let d = self.device.trim_start_matches("//");
        let d = d.rsplit('@').next().unwrap_or(d);
        let host = d.split([':', '/']).next()?;
        if host.is_empty() {
            None
        } else {
            Some(host.to_string())
        }
    }
}

/// Parse /proc/self/mountinfo into indexable + network mounts.
///
/// Pseudo filesystems (proc, sysfs, tmpfs, devtmpfs, overlay, ...) are
/// skipped: they are either unsupported or not user data. `/.snapshots`-style
/// exclusions are a policy decision for the caller, not the parser.
#[cfg(target_os = "linux")]
pub fn system_mounts() -> std::io::Result<Vec<MountInfo>> {
    let raw = std::fs::read_to_string("/proc/self/mountinfo")?;
    Ok(parse_mountinfo(&raw))
}

#[cfg(target_os = "linux")]
pub fn parse_mountinfo(raw: &str) -> Vec<MountInfo> {
    let mut out = Vec::new();
    for line in raw.lines() {
        // mountinfo: pre-separator fields, " - ", then: fstype source super-opts
        let Some((pre, post)) = line.split_once(" - ") else {
            continue;
        };
        let mut post_it = post.split_whitespace();
        let (Some(fstype), Some(dev)) = (post_it.next(), post_it.next()) else {
            continue;
        };
        let fs = FsKind::from_fstype(fstype);
        let interesting = !matches!(&fs, FsKind::Unsupported(_));
        if !interesting {
            continue;
        }
        // field 5 of the pre-separator part is the mount point
        let Some(mountpoint) = pre.split_whitespace().nth(4) else {
            continue;
        };
        // kernel escapes spaces etc. as \040-style octal
        let mountpoint = mountpoint.replace("\\040", " ");
        let source = match &fs {
            FsKind::Network(_) => MountSource::Remote {
                host: String::new(), // filled from device below
            },
            _ => MountSource::Local,
        };
        let mut mi = MountInfo {
            device: source_unescape(dev),
            mountpoint: PathBuf::from(mountpoint),
            fs,
            source,
        };
        if let MountSource::Remote { .. } = mi.source {
            match mi.network_host() {
                Some(host) => mi.source = MountSource::Remote { host },
                None => continue, // cannot identify a server to provision
            }
        }
        out.push(mi);
    }
    // Longest-mountpoint-first so callers can attribute paths to the
    // innermost mount.
    out.sort_by(|a, b| b.mountpoint.cmp(&a.mountpoint));
    out
}

#[cfg(target_os = "linux")]
fn source_unescape(s: &str) -> String {
    s.replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn parses_mixed_mountinfo() {
        let raw = "\
22 1 8:2 / / rw,relatime - ext4 /dev/sda2 rw
30 22 0:25 / /proc rw - proc proc rw
31 22 259:3 / /home rw,relatime - btrfs /dev/nvme0n1p3 rw,space_cache=v2
40 31 0:40 / /mnt/win rw - cifs //192.168.1.50/share rw
41 31 0:41 / /mnt/data rw - nfs4 nas:/export/data rw
42 22 0:42 / /mnt/ntfs rw - ntfs3 /dev/sdb1 rw
43 22 0:43 / /tank rw - zfs tank/data rw
";
        let mounts = parse_mountinfo(raw);
        assert_eq!(mounts.len(), 6);
        let kinds: Vec<_> = mounts.iter().map(|m| m.fs.clone()).collect();
        assert!(kinds.contains(&FsKind::Ext4));
        assert!(kinds.contains(&FsKind::Btrfs));
        assert!(kinds.contains(&FsKind::Ntfs));
        assert!(kinds.contains(&FsKind::Zfs));
        assert!(kinds.contains(&FsKind::Network("cifs".into())));
        assert!(kinds.contains(&FsKind::Network("nfs4".into())));
        let cifs = mounts
            .iter()
            .find(|m| m.fs == FsKind::Network("cifs".into()))
            .unwrap();
        assert_eq!(cifs.network_host().as_deref(), Some("192.168.1.50"));
        let nfs = mounts
            .iter()
            .find(|m| m.fs == FsKind::Network("nfs4".into()))
            .unwrap();
        assert_eq!(nfs.network_host().as_deref(), Some("nas"));
    }
}
