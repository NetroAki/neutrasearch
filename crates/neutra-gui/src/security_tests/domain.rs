use crate::mount_scope::{
    portable_path_in_root, record_in_roots, scope_within_selected_roots, select_mounts_for_roots,
};
use crate::transport::remote_failure_is_offline;
use neutra_core::MountInfo;
use std::path::PathBuf;

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
