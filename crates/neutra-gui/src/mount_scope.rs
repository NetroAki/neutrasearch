use neutra_core::MountInfo;
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::Command;

pub(crate) fn normalize_selected_root(root: PathBuf) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        let value = root.to_string_lossy();
        if let Some(path) = value.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{path}"));
        }
        if let Some(path) = value.strip_prefix(r"\\?\") {
            return PathBuf::from(path);
        }
    }
    root
}

pub(crate) fn portable_path_in_root(path: &str, root: &str, case_sensitive: bool) -> bool {
    let normalize = |value: &str| {
        let value = value.replace('\\', "/");
        if case_sensitive {
            value
        } else {
            value.to_ascii_lowercase()
        }
    };
    let path = normalize(path);
    let mut root = normalize(root);
    while root.len() > 3 && root.ends_with('/') && !root.as_bytes()[1].is_ascii_alphabetic() {
        root.pop();
    }
    while root.len() > 1 && root.ends_with('/') {
        root.pop();
    }
    path == root
        || (root.ends_with('/') && path.starts_with(&root))
        || path
            .strip_prefix(&root)
            .is_some_and(|tail| tail.starts_with('/'))
}

pub(crate) fn record_in_roots(path: &str, roots: &[PathBuf]) -> bool {
    let case_sensitive = cfg!(not(any(target_os = "windows", target_os = "macos")));
    roots
        .iter()
        .any(|root| portable_path_in_root(path, &root.to_string_lossy(), case_sensitive))
}

pub(crate) fn scope_within_selected_roots(scope: &str, selected_roots: &[PathBuf]) -> bool {
    let case_sensitive = cfg!(not(any(target_os = "windows", target_os = "macos")));
    selected_roots
        .iter()
        .any(|selected| portable_path_in_root(scope, &selected.to_string_lossy(), case_sensitive))
}

pub(crate) fn selected_scan_mounts(roots: &[PathBuf]) -> Vec<MountInfo> {
    if roots.is_empty() {
        return Vec::new();
    }
    #[cfg(target_os = "linux")]
    {
        let trusted = neutra_core::mounts::system_mounts().unwrap_or_default();
        return select_mounts_for_roots(roots, &trusted);
    }
    #[cfg(target_os = "windows")]
    {
        let mut mounts = Vec::new();
        for root in roots {
            let value = root.to_string_lossy().replace('/', "\\");
            let bytes = value.as_bytes();
            if bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && bytes[2] == b'\\'
            {
                let mountpoint = PathBuf::from(&value[..3]);
                if !mounts
                    .iter()
                    .any(|mount: &MountInfo| mount.mountpoint == mountpoint)
                {
                    mounts.push(requested_mount_with_filesystem(
                        mountpoint,
                        neutra_core::FsKind::Ntfs,
                    ));
                }
            }
        }
        return mounts;
    }
    #[cfg(target_os = "macos")]
    {
        let output = Command::new("/sbin/mount").output().ok();
        let trusted = output
            .filter(|output| output.status.success())
            .map(|output| parse_macos_mount_output(&String::from_utf8_lossy(&output.stdout)))
            .unwrap_or_default();
        return select_mounts_for_roots(roots, &trusted);
    }
    #[allow(unreachable_code)]
    Vec::new()
}

#[cfg(target_os = "macos")]
fn parse_macos_mount_output(output: &str) -> Vec<MountInfo> {
    output
        .lines()
        .filter_map(|line| {
            let (device, mounted) = line.split_once(" on ")?;
            let (mountpoint, options) = mounted.rsplit_once(" (")?;
            let filesystem = options.trim_end_matches(')').split(',').next()?.trim();
            if !matches!(filesystem, "apfs" | "hfs") || mountpoint.starts_with("/System/Volumes/") {
                return None;
            }
            Some(MountInfo {
                device: device.into(),
                mountpoint: PathBuf::from(mountpoint),
                fs: neutra_core::FsKind::Unsupported(filesystem.to_ascii_lowercase()),
                source: neutra_core::MountSource::Local,
            })
        })
        .collect()
}

#[cfg(target_os = "windows")]
fn requested_mount(mountpoint: PathBuf) -> MountInfo {
    MountInfo {
        device: mountpoint
            .to_string_lossy()
            .trim_end_matches('\\')
            .to_owned(),
        mountpoint,
        fs: neutra_core::FsKind::Ntfs,
        source: neutra_core::MountSource::Local,
    }
}

pub(crate) fn same_root(left: &Path, right: &Path) -> bool {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        left.to_string_lossy()
            .trim_end_matches(['/', '\\'])
            .eq_ignore_ascii_case(right.to_string_lossy().trim_end_matches(['/', '\\']))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        left == right
    }
}

pub(crate) fn default_system_roots() -> Vec<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        use neutra_core::FsKind;
        use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDriveStringsW};
        const DRIVE_FIXED: u32 = 3;
        const DRIVE_REMOVABLE: u32 = 2;
        let required = unsafe { GetLogicalDriveStringsW(0, std::ptr::null_mut()) };
        if required == 0 {
            return vec![PathBuf::from(r"C:\")];
        }
        let mut buffer = vec![0u16; required as usize + 1];
        let written = unsafe { GetLogicalDriveStringsW(buffer.len() as u32, buffer.as_mut_ptr()) };
        if written == 0 || written as usize >= buffer.len() {
            return vec![PathBuf::from(r"C:\")];
        }
        let mut roots = Vec::new();
        let mut offset = 0usize;
        while offset < written as usize {
            let Some(length) = buffer[offset..].iter().position(|value| *value == 0) else {
                break;
            };
            if length == 0 {
                break;
            }
            let root = &buffer[offset..offset + length + 1];
            offset += length + 1;
            if matches!(
                unsafe { GetDriveTypeW(root.as_ptr()) },
                DRIVE_FIXED | DRIVE_REMOVABLE
            ) {
                roots.push(PathBuf::from(String::from_utf16_lossy(&root[..length])));
            }
        }
        if roots.is_empty() {
            roots.push(PathBuf::from(r"C:\"));
        }
        roots
    }
    #[cfg(not(target_os = "windows"))]
    {
        vec![PathBuf::from("/")]
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn requested_mount_with_filesystem(
    mountpoint: PathBuf,
    fs: neutra_core::FsKind,
) -> MountInfo {
    MountInfo {
        device: String::new(),
        mountpoint,
        fs,
        source: neutra_core::MountSource::Local,
    }
}

#[cfg(any(not(target_os = "windows"), test))]
pub(crate) fn select_mounts_for_roots(roots: &[PathBuf], trusted: &[MountInfo]) -> Vec<MountInfo> {
    let full_machine = roots
        .iter()
        .any(|root| matches!(root.to_string_lossy().as_ref(), "/" | r"\"));
    let mut selected = Vec::<MountInfo>::new();
    if full_machine {
        for mount in trusted {
            if mount.fs.is_indexable_local()
                && !selected
                    .iter()
                    .any(|existing| existing.mountpoint == mount.mountpoint)
            {
                selected.push(mount.clone());
            }
        }
        return selected;
    }
    for root in roots {
        let Some(mount) = trusted
            .iter()
            .filter(|mount| root.starts_with(&mount.mountpoint))
            .max_by_key(|mount| mount.mountpoint.as_os_str().len())
        else {
            continue;
        };
        if mount.fs.is_indexable_local()
            && !selected
                .iter()
                .any(|existing| existing.mountpoint == mount.mountpoint)
        {
            selected.push(mount.clone());
        }
    }
    selected
}
