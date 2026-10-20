use super::domain::parse_index;
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
