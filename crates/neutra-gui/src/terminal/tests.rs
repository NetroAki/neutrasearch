use super::domain::parse_index;
use super::domain::scan_completion_error;
use super::view::{executable_name, parse_serve};
use std::path::PathBuf;

#[test]
fn companion_uses_product_prefix() {
    assert_eq!(
        executable_name("neutrasearch-query"),
        if cfg!(windows) {
            "neutrasearch-query.exe"
        } else {
            "neutrasearch-query"
        }
    );
}

#[test]
fn index_command_defaults_to_the_machine_index_location() {
    assert_eq!(parse_index(Vec::new()).unwrap(), None);
    assert_eq!(
        parse_index(vec!["--output".into(), "files.nsx".into()]).unwrap(),
        Some(PathBuf::from("files.nsx"))
    );
}

#[test]
fn index_command_has_no_root_or_depth_mode() {
    assert!(parse_index(vec!["/mnt/data".into()])
        .unwrap_err()
        .contains("full machine"));
    assert!(parse_index(vec!["--depth".into(), "2".into()])
        .unwrap_err()
        .contains("full machine"));
}

#[test]
fn serve_watch_has_explicit_source() {
    let (index, watch, source) = parse_serve(vec![
        "--index".into(),
        "files.nsx".into(),
        "--watch".into(),
        "/home".into(),
        "--source".into(),
        "4".into(),
    ])
    .unwrap();
    assert_eq!(index, PathBuf::from("files.nsx"));
    assert_eq!(watch, Some(PathBuf::from("/home")));
    assert_eq!(source, 4);
}

#[test]
fn any_native_lane_error_fails_the_index_command_even_when_another_mount_succeeds() {
    let error = scan_completion_error(2, 1).expect("partial scan must fail");
    assert!(error.contains("1 error(s)"));
    assert!(error.contains("previous index was kept"));
}

#[test]
fn index_command_requires_at_least_one_completed_mount() {
    assert!(scan_completion_error(0, 0).is_some());
    assert!(scan_completion_error(2, 0).is_none());
}
