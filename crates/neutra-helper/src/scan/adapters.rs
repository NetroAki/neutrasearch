use anyhow::{Context, Result};
#[cfg(any(test, target_os = "windows", target_os = "macos"))]
use neutra_core::mounts::FsKind;
use neutra_core::mounts::MountInfo;

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
fn same_mountpoint(left: &std::path::Path, right: &std::path::Path) -> bool {
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
    let Some(roots) = logical_drive_roots() else {
        return Vec::new();
    };
    roots
        .into_iter()
        .filter_map(inspect_windows_drive)
        .collect()
}

#[cfg(target_os = "windows")]
fn logical_drive_roots() -> Option<Vec<Vec<u16>>> {
    #[allow(non_snake_case)]
    #[link(name = "kernel32")]
    extern "system" {
        fn GetLogicalDriveStringsW(length: u32, buffer: *mut u16) -> u32;
    }
    let required = run_bounded(
        "GetLogicalDriveStringsW(length)",
        std::time::Duration::from_millis(500),
        || unsafe { GetLogicalDriveStringsW(0, std::ptr::null_mut()) },
    )?;
    if required == 0 {
        return None;
    }
    let capacity = required as usize + 1;
    let result = run_bounded(
        "GetLogicalDriveStringsW(values)",
        std::time::Duration::from_millis(500),
        move || {
            let mut buffer = vec![0u16; capacity];
            let written =
                unsafe { GetLogicalDriveStringsW(buffer.len() as u32, buffer.as_mut_ptr()) };
            (written, buffer)
        },
    )?;
    let (written, buffer) = result;
    if written == 0 || written as usize >= buffer.len() {
        return None;
    }
    let mut roots = Vec::new();
    let mut offset = 0;
    while offset < written as usize {
        let Some(length) = buffer[offset..].iter().position(|unit| *unit == 0) else {
            break;
        };
        if length == 0 {
            break;
        }
        roots.push(buffer[offset..offset + length + 1].to_vec());
        offset += length + 1;
    }
    Some(roots)
}

#[cfg(target_os = "windows")]
fn inspect_windows_drive(root: Vec<u16>) -> Option<MountInfo> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    #[allow(non_snake_case)]
    #[link(name = "kernel32")]
    extern "system" {
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
    let drive_root = root.clone();
    let drive_type = run_bounded(
        "GetDriveTypeW",
        std::time::Duration::from_millis(500),
        move || unsafe { GetDriveTypeW(drive_root.as_ptr()) },
    )?;
    if !matches!(drive_type, DRIVE_FIXED | DRIVE_REMOVABLE) {
        return None;
    }
    let volume_root = root.clone();
    let (ok, filesystem) = run_bounded(
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
    )?;
    if ok == 0 {
        return None;
    }
    let length = filesystem
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(filesystem.len());
    if !String::from_utf16_lossy(&filesystem[..length]).eq_ignore_ascii_case("ntfs") {
        return None;
    }
    let root_len = root.len().saturating_sub(1);
    let mountpoint = std::path::PathBuf::from(OsString::from_wide(&root[..root_len]));
    let device = mountpoint
        .to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .to_owned();
    Some(MountInfo {
        device,
        mountpoint,
        fs: FsKind::Ntfs,
        source: neutra_core::MountSource::Local,
    })
}

#[cfg(not(target_os = "windows"))]
fn same_mountpoint(left: &std::path::Path, right: &std::path::Path) -> bool {
    left == right
}
