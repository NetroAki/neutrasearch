#[cfg(unix)]
use crate::transport::validate_elevated_helper;
use crate::transport::{helper_start_failure, select_helper};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

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
