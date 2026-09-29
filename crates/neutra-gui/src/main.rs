mod app;
mod terminal;
mod transport;
mod ui;

pub(crate) use app::{Event, GuiSettings, LaneState, NeutraApp};
pub(crate) use transport::{
    launch_file_action, scan_has_reachable_lane, spawn_local_helper,
    FileAction,
};
#[cfg(test)]
use transport::{
    helper_start_failure, remote_failure_is_offline, select_helper, validate_elevated_helper,
};

use neutra_core::MountInfo;
use std::io::Write;
use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::process::Command;
#[cfg(test)]
use std::sync::{Arc, Mutex};

/// Result caps: the home view shows the newest slice of the index and typed
/// searches stay at the interactive cap. The status bar reports the full
/// matched count either way ("1,000 of 40,312 results"), so nothing is hidden.
pub(crate) const HOME_RESULT_CAP: usize = 10_000;
pub(crate) const TYPED_RESULT_CAP: usize = 1_000;

fn embedded_logo() -> (Vec<u8>, u32, u32) {
    let image = image::load_from_memory(include_bytes!("../assets/neutrasearch.png"))
        .expect("embedded Neutrasearch icon must decode")
        .into_rgba8();
    let (width, height) = image.dimensions();
    (image.into_raw(), width, height)
}

fn app_icon() -> egui::IconData {
    let (rgba, width, height) = embedded_logo();
    egui::IconData {
        rgba,
        width,
        height,
    }
}

fn main() -> eframe::Result<()> {
    match terminal::action() {
        terminal::Action::Gui => {}
        terminal::Action::Exit(code) => std::process::exit(code),
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([760.0, 500.0])
            .with_title("Neutrasearch")
            .with_app_id("neutrasearch")
            .with_icon(app_icon()),
        renderer: eframe::Renderer::Glow,
        vsync: false,
        ..Default::default()
    };
    eframe::run_native(
        "Neutrasearch",
        options,
        Box::new(|cc| Ok(Box::new(app::NeutraApp::new(cc)))),
    )
}

#[cfg(not(target_os = "windows"))]
fn request_elevated_restart() -> Result<(), String> {
    Err("elevated restart is only available on Windows".into())
}

#[cfg(not(unix))]
fn validate_elevated_helper(path: &std::path::Path) -> Result<PathBuf, String> {
    std::fs::canonicalize(path).map_err(|error| {
        format!(
            "cannot resolve installed helper {}: {error}",
            path.display()
        )
    })
}

fn normalize_selected_root(root: PathBuf) -> PathBuf {
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

fn portable_path_in_root(path: &str, root: &str, case_sensitive: bool) -> bool {
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

fn record_in_roots(path: &str, roots: &[PathBuf]) -> bool {
    let case_sensitive = cfg!(not(any(target_os = "windows", target_os = "macos")));
    roots
        .iter()
        .any(|root| portable_path_in_root(path, &root.to_string_lossy(), case_sensitive))
}

fn scope_within_selected_roots(scope: &str, selected_roots: &[PathBuf]) -> bool {
    let case_sensitive = cfg!(not(any(target_os = "windows", target_os = "macos")));
    selected_roots
        .iter()
        .any(|selected| portable_path_in_root(scope, &selected.to_string_lossy(), case_sensitive))
}

fn selected_scan_mounts(roots: &[PathBuf]) -> Vec<MountInfo> {
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

fn gui_settings_path() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(std::env::temp_dir)
            .join("Neutrasearch/gui-settings.json")
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .map(|home| home.join("Library/Application Support"))
            .unwrap_or_else(std::env::temp_dir)
            .join("Neutrasearch/gui-settings.json")
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .filter(|path| path.is_absolute())
                    .map(|home| home.join(".config"))
            })
            .unwrap_or_else(std::env::temp_dir)
            .join("neutrasearch/gui-settings.json")
    }
}

fn load_gui_settings(path: &std::path::Path) -> Option<GuiSettings> {
    let bytes = std::fs::read(path).ok()?;
    let mut settings: GuiSettings = serde_json::from_slice(&bytes).ok()?;
    settings.roots = settings
        .roots
        .into_iter()
        .map(normalize_selected_root)
        .filter(|root| root.is_absolute())
        .collect();
    settings.roots.sort();
    settings.roots.dedup_by(|left, right| same_root(left, right));
    Some(settings)
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
        device: mountpoint.to_string_lossy().trim_end_matches('\\').to_owned(),
        mountpoint,
        fs: neutra_core::FsKind::Ntfs,
        source: neutra_core::MountSource::Local,
    }
}

fn same_root(left: &PathBuf, right: &PathBuf) -> bool {
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

fn default_system_roots() -> Vec<PathBuf> {
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
fn requested_mount_with_filesystem(mountpoint: PathBuf, fs: neutra_core::FsKind) -> MountInfo {
    MountInfo {
        device: String::new(),
        mountpoint,
        fs,
        source: neutra_core::MountSource::Local,
    }
}

#[cfg(any(not(target_os = "windows"), test))]
fn select_mounts_for_roots(roots: &[PathBuf], trusted: &[MountInfo]) -> Vec<MountInfo> {
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

fn save_gui_settings(path: &std::path::Path, settings: &GuiSettings) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "settings path has no parent".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create settings directory: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("cannot protect settings directory: {error}"))?;
    }
    let temporary = path.with_extension("json.new");
    let bytes = serde_json::to_vec_pretty(settings)
        .map_err(|error| format!("cannot encode settings: {error}"))?;
    let _ = std::fs::remove_file(&temporary);
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|error| format!("cannot create settings: {error}"))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("cannot write settings: {error}"))?;
    drop(file);
    if path.exists() {
        std::fs::remove_file(path)
            .map_err(|error| format!("cannot replace settings: {error}"))?;
    }
    publish_settings(&temporary, path)
        .map_err(|error| format!("cannot publish settings: {error}"))
}

#[cfg(not(target_os = "windows"))]
fn publish_settings(
    temporary: &std::path::Path,
    destination: &std::path::Path,
) -> std::io::Result<()> {
    std::fs::rename(temporary, destination)
}

#[cfg(target_os = "windows")]
fn publish_settings(
    temporary: &std::path::Path,
    destination: &std::path::Path,
) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    extern "system" {
        fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
    }
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    let existing = temporary
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let replacement = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        MoveFileExW(
            existing.as_ptr(),
            replacement.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

 fn env_flag(name: &str) -> bool {
     std::env::var_os(name).is_some()
 }
 fn configured_index() -> Option<PathBuf> {
     std::env::var_os("NEUTRASEARCH_INDEX").map(PathBuf::from)
 }
fn legacy_cache_path() -> PathBuf {
    if let Some(path) = configured_index() {
        return path;
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(std::env::temp_dir)
            .join("Neutrasearch/index.bin")
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .map(|home| home.join("Library/Caches"))
            .unwrap_or_else(std::env::temp_dir)
            .join("Neutrasearch/index.bin")
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .filter(|path| path.is_absolute())
                    .map(|home| home.join(".cache"))
            })
            .unwrap_or_else(std::env::temp_dir)
            .join("neutrasearch/index.bin")
    }
}
fn compact_cache_path() -> PathBuf {
    neutra_core::paths::resolve_index_path(None)
}

#[cfg(test)]
mod security_tests {
    use super::*;

    #[test]
    fn embedded_logo_decodes_to_a_complete_rgba_icon() {
        let icon = app_icon();
        assert_eq!((icon.width, icon.height), (128, 128));
        assert_eq!(icon.rgba.len(), 128 * 128 * 4);
    }

    #[test]
    fn elevated_helper_cannot_come_from_environment_override() {
        let error = select_helper(
            Some(PathBuf::from("/tmp/untrusted-helper")),
            Some(PathBuf::from("/usr/bin/neutrasearch")),
            true,
        )
        .unwrap_err();
        assert!(error.contains("refusing to elevate"));
    }

    #[test]
    #[cfg(unix)]
    fn elevated_helper_rejects_paths_outside_system_allowlist() {
        use std::os::unix::fs::PermissionsExt;
        let directory = std::env::temp_dir().join(format!(
            "neutrasearch-untrusted-helper-{}",
            std::process::id()
        ));
        let helper = directory.join("neutrasearch-helper");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(&helper, b"fixture").unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(validate_elevated_helper(&helper).is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn normal_helper_override_remains_available_for_development() {
        let helper = select_helper(
            Some(PathBuf::from("custom-helper")),
            Some(PathBuf::from("neutrasearch")),
            false,
        )
        .unwrap();
        assert_eq!(helper, PathBuf::from("custom-helper"));
    }

    #[test]
    fn missing_desktop_authorization_becomes_an_actionable_error() {
        let stderr = Arc::new(Mutex::new(
            "Error creating textual authentication agent: Error opening current controlling terminal"
                .to_owned(),
        ));
        let error = helper_start_failure("handshake failed", &stderr);
        assert!(error.contains("launch Neutrasearch from the desktop"));
        assert!(!error.contains("handshake"));
    }

    #[test]
    fn selected_roots_include_descendants_but_not_prefix_siblings() {
        let roots = vec![PathBuf::from("/home/alex/Documents")];
        assert!(record_in_roots("/home/alex/Documents/report.pdf", &roots));
        assert!(record_in_roots("/home/alex/Documents", &roots));
        assert!(!record_in_roots(
            "/home/alex/Documents-old/report.pdf",
            &roots
        ));
        assert!(!record_in_roots("/home/alex/Documents/report.pdf", &[]));
        assert!(portable_path_in_root("/Users/alex/report.pdf", "/", false));
        assert!(portable_path_in_root(
            r"C:\Users\alex\report.pdf",
            r"C:\",
            false
        ));
        assert!(!portable_path_in_root(r"D:\report.pdf", r"C:\", false));
        assert!(scope_within_selected_roots(
            "/home/alex/Documents/reports",
            &[PathBuf::from("/home/alex/Documents")]
        ));
        assert!(!scope_within_selected_roots(
            "/home",
            &[PathBuf::from("/home/alex/Documents")]
        ));
    }

    #[test]
    fn full_machine_root_selects_every_supported_local_mount() {
        let trusted = vec![
            MountInfo {
                device: "/dev/root".into(),
                mountpoint: "/".into(),
                fs: neutra_core::FsKind::Ext4,
                source: neutra_core::MountSource::Local,
            },
            MountInfo {
                device: "/dev/home".into(),
                mountpoint: "/home".into(),
                fs: neutra_core::FsKind::Btrfs,
                source: neutra_core::MountSource::Local,
            },
            MountInfo {
                device: "/dev/data".into(),
                mountpoint: "/mnt/data".into(),
                fs: neutra_core::FsKind::Ext4,
                source: neutra_core::MountSource::Local,
            },
            MountInfo {
                device: "nas:/share".into(),
                mountpoint: "/mnt/team".into(),
                fs: neutra_core::FsKind::Network("nfs4".into()),
                source: neutra_core::MountSource::Remote { host: "nas".into() },
            },
        ];
        let selected = select_mounts_for_roots(&[PathBuf::from("/")], &trusted);
        let mountpoints: Vec<_> = selected
            .iter()
            .map(|mount| mount.mountpoint.clone())
            .collect();
        assert_eq!(
            mountpoints,
            vec![
                PathBuf::from("/"),
                PathBuf::from("/home"),
                PathBuf::from("/mnt/data")
            ]
        );
    }

    #[test]
    fn network_roots_do_not_fall_back_to_scanning_the_parent_local_volume() {
        let trusted = vec![
            MountInfo {
                device: "server:/share".into(),
                mountpoint: "/mnt/team".into(),
                fs: neutra_core::FsKind::Network("nfs4".into()),
                source: neutra_core::MountSource::Remote {
                    host: "server".into(),
                },
            },
            MountInfo {
                device: "/dev/root".into(),
                mountpoint: "/".into(),
                fs: neutra_core::FsKind::Ext4,
                source: neutra_core::MountSource::Local,
            },
        ];
        assert!(select_mounts_for_roots(&[PathBuf::from("/mnt/team/docs")], &trusted).is_empty());
        let local = select_mounts_for_roots(&[PathBuf::from("/home/alex")], &trusted);
        assert_eq!(local.len(), 1);
        assert_eq!(local[0].mountpoint, PathBuf::from("/"));
    }

    #[test]
    fn successful_empty_scans_still_replace_stale_results() {
        assert!(scan_has_reachable_lane(1, 0));
        assert!(scan_has_reachable_lane(3, 1));
        assert!(!scan_has_reachable_lane(3, 3));
        assert!(!scan_has_reachable_lane(0, 0));
    }

    #[test]
    fn offline_network_errors_are_retryable_but_integrity_failures_are_not() {
        assert!(remote_failure_is_offline(&anyhow::anyhow!(
            "ssh: connect to host studio: Connection timed out"
        )));
        assert!(!remote_failure_is_offline(&anyhow::anyhow!(
            "helper checksum mismatch"
        )));
        assert!(!remote_failure_is_offline(&anyhow::anyhow!(
            "Permission denied (publickey)"
        )));
    }

    #[test]
    fn gui_settings_roundtrip_preserves_completed_onboarding_and_roots() {
        let directory =
            std::env::temp_dir().join(format!("neutrasearch-gui-settings-{}", std::process::id()));
        let path = directory.join("gui-settings.json");
        let _ = std::fs::remove_dir_all(&directory);
        let root = std::env::temp_dir();
        let settings = GuiSettings {
            onboarding_complete: true,
            roots: vec![root.clone()],
            ..Default::default()
        };
        save_gui_settings(&path, &settings).unwrap();
        let incomplete = GuiSettings {
            onboarding_complete: false,
            roots: vec![root.clone()],
            ..Default::default()
        };
        save_gui_settings(&path, &incomplete).unwrap();
        save_gui_settings(&path, &settings).unwrap();
        let loaded = load_gui_settings(&path).unwrap();
        assert!(loaded.onboarding_complete);
        assert_eq!(loaded.roots, vec![root]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        std::fs::remove_dir_all(directory).unwrap();
    }
}
