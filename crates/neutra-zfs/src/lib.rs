//! ZFS lane.
//!
//! OpenZFS exposes no stable userspace metadata-enumeration API: the intended
//! production path is snapshot + DMU/ZAP enumeration through libzpool, which
//! only exists inside an OpenZFS build tree (`zfs-libzpool` feature; the
//! default build refuses to pretend an unverified ABI is safe). `zdb` output
//! is deliberately never parsed: it is a debugging interface, not a
//! production API.
//!
//! What this crate provides instead:
//! - a tested `zfs diff` parser plus snapshot command builders for *updates*,
//! - an explicit, opt-in single-pass enumeration for the *initial* index
//!   (`NEUTRASEARCH_ZFS_ALLOW_WALK`). It is a visible fallback, never a
//!   silent one: the flag is documented in the refusal message, and the
//!   pass is one sequential openat/getdents64/fstatat sweep that never
//!   follows symlinks and never crosses filesystem boundaries.

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

/// Initial ZFS scan.
///
/// Default: refuse with the exact remedy. Opt-in: one visible single-pass
/// enumeration (`NEUTRASEARCH_ZFS_ALLOW_WALK`), intended for desktop volumes
/// where a one-time index build matters more than the no-walk guarantee.
pub fn scan(mount: &MountInfo, sink: &mut dyn FnMut(FileRecord)) -> Result<ScanStats> {
    #[cfg(feature = "zfs-libzpool")]
    {
        return libzpool_backend::scan(mount, sink);
    }
    #[cfg(all(not(feature = "zfs-libzpool"), target_os = "linux"))]
    {
        if env_present("NEUTRASEARCH_ZFS_ALLOW_WALK", "NEUTRA_ZFS_ALLOW_WALK") {
            let mut stats = enumerate::scan_tree(&mount.mountpoint, sink)?;
            stats.detail = "single-pass enumeration (opt-in; libzpool ZAP is the native lane)".into();
            return Ok(stats);
        }
        bail!(
            "ZFS has no stable userspace metadata API in this build: set \
             NEUTRASEARCH_ZFS_ALLOW_WALK=1 for a one-time single-pass enumeration, or rebuild \
             with --features neutra-zfs/zfs-libzpool against an OpenZFS build tree"
        )
    }
    #[cfg(all(not(feature = "zfs-libzpool"), not(target_os = "linux")))]
    {
        let _ = (mount, sink);
        bail!(
            "ZFS single-pass enumeration requires Linux; rebuild with --features \
             neutra-zfs/zfs-libzpool against an OpenZFS build tree for the native lane"
        )
    }
}

fn env_present(current: &str, legacy: &str) -> bool {
    std::env::var_os(current).is_some() || std::env::var_os(legacy).is_some()
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
    /// The opt-in single-pass enumeration is compiled in (Linux builds).
    pub walk_lane_compiled: bool,
    /// The explicit opt-in is currently enabled for this process.
    pub walk_opt_in: bool,
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
        walk_lane_compiled: cfg!(all(not(feature = "zfs-libzpool"), target_os = "linux")),
        walk_opt_in: env_present("NEUTRASEARCH_ZFS_ALLOW_WALK", "NEUTRA_ZFS_ALLOW_WALK"),
    }
}

/// One sequential openat/getdents64/fstatat sweep. Never follows symlinks and
/// never descends across filesystem boundaries (st_dev), so a nested mount is
/// recorded as one entry and its contents left to its own lane.
#[cfg(target_os = "linux")]
mod enumerate {
    use super::*;
    use anyhow::Context as _;
    use std::collections::HashSet;
    use std::ffi::{CStr, CString, OsString};
    use std::fs::File;
    use std::os::fd::{AsRawFd, FromRawFd, RawFd};
    use std::os::unix::ffi::OsStringExt;
    use std::time::Instant;

    #[repr(C)]
    struct LinuxDirent64 {
        d_ino: u64,
        d_off: i64,
        d_reclen: u16,
        d_type: u8,
        // NUL-terminated name follows; parsed via CStr.
    }

    const BUFFER_BYTES: usize = 64 * 1024;

    pub(super) fn scan_tree(
        root: &Path,
        sink: &mut dyn FnMut(FileRecord),
    ) -> Result<ScanStats> {
        let started = Instant::now();
        let prefix = root.to_string_lossy().trim_end_matches('/').to_owned();
        let root_c = CString::new(root.as_os_str().as_encoded_bytes())
            .map_err(|_| anyhow::anyhow!("dataset path contains NUL"))?;
        let root_fd = open_dir_fd(root_c.as_ptr())?;
        let root_dev = {
            let mut stat: libc::stat = unsafe { std::mem::zeroed() };
            if unsafe { libc::fstat(root_fd, &mut stat) } != 0 {
                return Err(std::io::Error::last_os_error()).context("stat ZFS dataset root");
            }
            stat.st_dev
        };
        // File owns the descriptor from here.
        let mut queue = std::collections::VecDeque::new();
        // SAFETY: root_fd is an owned descriptor from open_dir_fd.
        let root_file = unsafe { File::from_raw_fd(root_fd) };
        queue.push_back((DirHandle { fd: root_file }, prefix.clone()));
        let mut stats = ScanStats::default();
        let mut buffer = vec![0u8; BUFFER_BYTES];
        let mut visited = HashSet::new();

        while let Some((dir, dir_path)) = queue.pop_front() {
            loop {
                // SAFETY: fd is an open directory; buffer is writable for the
                // syscall length and getdents64 returns bytes written.
                let bytes = unsafe {
                    libc::syscall(
                        libc::SYS_getdents64,
                        dir.fd.as_raw_fd(),
                        buffer.as_mut_ptr(),
                        buffer.len(),
                    )
                };
                if bytes < 0 {
                    let error = std::io::Error::last_os_error();
                    if error.kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(error).context("enumerate ZFS dataset");
                }
                if bytes == 0 {
                    break;
                }
                let bytes = bytes as usize;
                let mut offset = 0usize;
                while offset < bytes {
                    // SAFETY: getdents64 returned complete records within the buffer.
                    let entry = unsafe {
                        &*(buffer.as_ptr().add(offset) as *const LinuxDirent64)
                    };
                    let reclen = entry.d_reclen as usize;
                    if reclen == 0 || offset + reclen > bytes {
                        anyhow::bail!("corrupt getdents64 record");
                    }
                    offset += reclen;
                                        // linux_dirent64: d_ino(8) d_off(8) d_reclen(2) d_type(1) d_name[]
                    let name = unsafe {
                        CStr::from_ptr(buffer.as_ptr().add(offset - reclen + 19).cast())
                    };
                    let name = name.to_bytes();
                    if name != b"." && name != b".." {
                        emit_entry(
                            &dir.fd,
                            &dir_path,
                            &prefix,
                            root_dev,
                            &mut visited,
                            &mut queue,
                            name,
                            &mut stats,
                            sink,
                        )?;
                    }
                }
            }
        }
        stats.wall_ms = started.elapsed().as_millis() as u64;
        Ok(stats)
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_entry(
        parent_fd: &File,
        dir_path: &str,
        prefix: &str,
        root_dev: libc::dev_t,
        visited: &mut HashSet<u64>,
        queue: &mut std::collections::VecDeque<(DirHandle, String)>,
        name: &[u8],
        stats: &mut ScanStats,
        sink: &mut dyn FnMut(FileRecord),
    ) -> Result<()> {
        let cname = CString::new(name).map_err(|_| anyhow::anyhow!("entry name contains NUL"))?;
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: cname is NUL-terminated and parent_fd is an open directory.
        if unsafe { libc::fstatat(parent_fd.as_raw_fd(), cname.as_ptr(), &mut stat, libc::AT_SYMLINK_NOFOLLOW) } != 0 {
            // Raced deletions are skipped, not fatal: enumeration is a snapshot
            // of whatever the dataset looked like mid-sweep.
            return Ok(());
        }
        let name = OsString::from_vec(name.to_vec());
        let path = if dir_path == prefix {
            format!("{prefix}/{}", name.to_string_lossy())
        } else {
            format!("{dir_path}/{}", name.to_string_lossy())
        };
        let is_dir = stat.st_mode & libc::S_IFMT == libc::S_IFDIR;
        let kind = if is_dir {
            neutra_core::FileKind::Dir
        } else if stat.st_mode & libc::S_IFMT == libc::S_IFLNK {
            neutra_core::FileKind::Symlink
        } else if stat.st_mode & libc::S_IFMT == libc::S_IFREG {
            neutra_core::FileKind::File
        } else {
            neutra_core::FileKind::Other
        };
        if is_dir && stat.st_dev == root_dev && visited.insert(stat.st_ino) {
            // O_NOFOLLOW keeps directory descent symlink-safe.
            let child = openat_dir_fd(parent_fd.as_raw_fd(), cname.as_ptr())?;
            if child >= 0 {
                queue.push_back((DirHandle { fd: unsafe { File::from_raw_fd(child) } }, path.clone()));
            }
            stats.dirs += 1;
        } else if is_dir {
            stats.dirs += 1;
        } else {
            stats.files += 1;
        }
        sink(FileRecord {
            path: path.into_boxed_str(),
            size: stat.st_size.max(0) as u64,
            disk: (stat.st_blocks.max(0) as u64).saturating_mul(512),
            mtime: stat.st_mtime,
            mode: stat.st_mode as u32,
            kind,
            fs: neutra_core::FsKind::Zfs,
            native_id: stat.st_ino,
            native_parent: 0,
            source: 0,
        });
        stats.records += 1;
        Ok(())
    }

    struct DirHandle {
        fd: File,
    }

    fn open_dir_fd(path: *const libc::c_char) -> Result<RawFd> {
        // SAFETY: path is a valid NUL-terminated string for the call.
        let fd = unsafe {
            libc::open(path, libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW)
        };
        if fd < 0 {
            Err(std::io::Error::last_os_error()).context("open ZFS dataset root")
        } else {
            Ok(fd)
        }
    }

    fn openat_dir_fd(parent: RawFd, name: *const libc::c_char) -> Result<RawFd> {
        // SAFETY: parent is an open descriptor and name is NUL-terminated.
        let fd = unsafe {
            libc::openat(parent, name, libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW)
        };
        if fd < 0 {
            Err(std::io::Error::last_os_error()).context("descend ZFS directory")
        } else {
            Ok(fd)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn record_path(record: &FileRecord) -> &str {
            record.path.as_ref()
        }

        #[test]
        fn sweep_enumerates_nested_tree_without_crossing_or_following() {
            let base = std::env::temp_dir().join(format!("neutra-zfs-walk-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&base);
            std::fs::create_dir_all(base.join("sub/deep")).unwrap();
            std::fs::write(base.join("top.txt"), b"hello").unwrap();
            std::fs::write(base.join("sub/data.bin"), [0u8; 11]).unwrap();
            std::fs::write(base.join("sub/deep/leaf.rs"), b"fn main() {}").unwrap();
            #[cfg(unix)]
            std::os::unix::fs::symlink("top.txt", base.join("alias.lnk")).unwrap();

            let mut records = Vec::new();
            let stats = scan_tree(&base, &mut |record| records.push(record)).unwrap();
            let mut paths = records.iter().map(record_path).collect::<Vec<_>>();
            paths.sort();
            let base = base.to_string_lossy().to_string();
            assert!(paths.contains(&format!("{base}/top.txt").as_str()));
            assert!(paths.contains(&format!("{base}/sub/deep/leaf.rs").as_str()));
            assert!(paths.contains(&format!("{base}/alias.lnk").as_str()));
            // sub and deep; the dataset root itself is not a record, matching
            // the NTFS and macOS lanes.
            assert_eq!(stats.dirs, 2);
            assert_eq!(stats.files, 4);
            let link = records.iter().find(|r| r.kind == neutra_core::FileKind::Symlink).unwrap();
            assert_eq!(link.size, 7); // lstat: symlink size is the target-name length ("top.txt")
            let leaf = records.iter().find(|r| record_path(r).ends_with("leaf.rs")).unwrap();
            assert_eq!(leaf.size, 12);
            assert_eq!(leaf.fs, neutra_core::FsKind::Zfs);
            let _ = std::fs::remove_dir_all(&base);
        }
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
