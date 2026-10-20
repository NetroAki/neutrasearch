use super::*;
#[cfg(any(target_os = "windows", target_os = "macos"))]
use crate::scan::discover_local_mounts;
use crate::scan::exclusion_prefixes;

#[test]
fn macos_mount_parser_keeps_user_visible_native_volumes() {
    let mounts = parse_macos_mount_output(
        "/dev/disk3s1s1 on / (apfs, sealed, local)\n\
         /dev/disk3s5 on /System/Volumes/Data (apfs, local)\n\
         /dev/disk7s1 on /Volumes/Archive Drive (hfs, local)\n\
         server:/share on /Volumes/Team (nfs, nodev)\n",
    );
    assert_eq!(mounts.len(), 2);
    assert_eq!(mounts[0].mountpoint, std::path::Path::new("/"));
    assert_eq!(
        mounts[1].mountpoint,
        std::path::Path::new("/Volumes/Archive Drive")
    );
    assert_eq!(mounts[1].fs, FsKind::Unsupported("hfs".into()));
}

#[test]
fn scan_requests_use_trusted_mount_metadata() {
    let trusted = MountInfo {
        device: "/dev/trusted".into(),
        mountpoint: "/mnt/data".into(),
        fs: FsKind::Ext4,
        source: neutra_core::MountSource::Local,
    };
    let spoofed = MountInfo {
        device: "/dev/evil".into(),
        mountpoint: "/mnt/data".into(),
        fs: FsKind::Ntfs,
        source: neutra_core::MountSource::Local,
    };
    assert!(resolve_scan_mounts(Vec::new(), vec![trusted.clone()], true)
        .unwrap()
        .is_empty());
    let resolved = resolve_scan_mounts(vec![spoofed], vec![trusted], true).unwrap();
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].device, "/dev/trusted");
    assert!(matches!(resolved[0].fs, FsKind::Ext4));
    assert!(resolve_scan_mounts(
        vec![MountInfo {
            mountpoint: "/unknown".into(),
            device: "/dev/evil".into(),
            fs: FsKind::Ntfs,
            source: neutra_core::MountSource::Local,
        }],
        Vec::new(),
        true,
    )
    .is_err());
}

#[test]
fn bounded_call_returns_before_a_slow_operation_finishes() {
    let started = std::time::Instant::now();
    let result = run_bounded(
        "test operation",
        std::time::Duration::from_millis(10),
        || {
            std::thread::sleep(std::time::Duration::from_secs(1));
            42
        },
    );
    assert_eq!(result, None);
    assert!(started.elapsed() < std::time::Duration::from_millis(500));
}

#[cfg(target_os = "windows")]
#[test]
fn windows_discovery_reports_only_real_ntfs_volume_roots() {
    let mounts = discover_local_mounts();
    assert!(
        !mounts.is_empty(),
        "Windows CI host should expose its system volume"
    );
    assert!(mounts.iter().all(|mount| mount.fs == FsKind::Ntfs));
    assert!(mounts.iter().all(|mount| mount.mountpoint.is_absolute()));
}

#[cfg(target_os = "macos")]
#[test]
fn macos_discovery_includes_the_user_visible_root_volume() {
    let mounts = discover_local_mounts();
    assert!(
        mounts
            .iter()
            .any(|mount| mount.mountpoint == std::path::Path::new("/")),
        "macOS CI host should expose its root APFS volume"
    );
}

#[test]
fn default_snapshot_directory_is_excluded_by_component() {
    let excluded = |mountpoint: &str, path: &str| {
        exclusion_prefixes(std::path::Path::new(mountpoint))
            .iter()
            .any(|prefix| path_has_component_prefix(path, prefix))
    };
    assert!(excluded("/", "/.snapshots/42/file"));
    assert!(excluded("/home", "/home/.snapshots/42/file"));
    assert!(!excluded("/", "/.snapshots-old/file"));
    assert!(excluded("/", "/proc/self/status"));
    assert!(excluded("/", "/sys/kernel"));
    assert!(!excluded("/home", "/home/system/file"));
}

#[test]
fn approved_scan_roots_are_bounded_to_requested_mounts() {
    let (device, mountpoint, root, inside, sibling) = if cfg!(target_os = "windows") {
        (
            r"C:",
            r"C:\",
            r"C:\Users\alex\Documents",
            r"C:\Users\alex\Documents\report.pdf",
            r"C:\Users\alex\Documents-old\report.pdf",
        )
    } else {
        (
            "/dev/root",
            "/",
            "/home/alex/Documents",
            "/home/alex/Documents/report.pdf",
            "/home/alex/Documents-old/report.pdf",
        )
    };
    let mount = MountInfo {
        device: device.into(),
        mountpoint: mountpoint.into(),
        fs: FsKind::Btrfs,
        source: neutra_core::MountSource::Local,
    };
    let roots = validate_scan_roots(vec![root.into()], std::slice::from_ref(&mount)).unwrap();
    assert!(portable_path_in_root(inside, &roots[0]));
    assert!(!portable_path_in_root(sibling, &roots[0]));
    assert!(validate_scan_roots(Vec::new(), std::slice::from_ref(&mount)).is_err());
    assert!(validate_scan_roots(vec!["relative".into()], &[mount]).is_err());
}
