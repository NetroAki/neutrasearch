use super::*;
use neutra_core::Query;
use widgets::{civil_date, normalize_path};

#[test]
fn civil_dates_cover_epoch_and_recent_values() {
    assert_eq!(civil_date(0), (1970, 1, 1));
    assert_eq!(civil_date(20_454), (2026, 1, 1));
}

#[test]
fn path_helpers_preserve_unix_windows_and_unc_hierarchies() {
    assert_eq!(
        normalize_path(r"/home/alex//Documents/"),
        "/home/alex/Documents"
    );
    assert_eq!(
        parent_path("/home/alex/Documents/file.txt"),
        "/home/alex/Documents"
    );
    assert_eq!(
        ancestor_paths("/home/alex"),
        vec!["/", "/home", "/home/alex"]
    );

    assert_eq!(normalize_path(r"C:\Users\Alex\"), "C:/Users/Alex");
    assert_eq!(parent_path(r"C:\Users\Alex"), "C:/Users");
    assert_eq!(
        ancestor_paths(r"C:\Users\Alex"),
        vec!["/", "C:/", "C:/Users", "C:/Users/Alex"]
    );
    assert_eq!(path_name("C:/"), "C:");

    assert_eq!(
        normalize_path(r"\\server\share\Folder\file.txt"),
        "//server/share/Folder/file.txt"
    );
    assert_eq!(parent_path("//server/share"), "/");
    assert_eq!(
        ancestor_paths("//server/share/Folder"),
        vec!["/", "//server/share", "//server/share/Folder"]
    );
}

use fixtures::dense_unicode_index;
#[path = "fixtures.rs"]
mod fixtures;

#[test]
fn reference_index_exercises_dense_views_and_unicode_fallbacks() {
    let index = dense_unicode_index();
    assert!(index.len() >= 60);
    let mut query = Query::parse("");
    query.limit = 1_000;
    let (hits, _) = index.search(&query).unwrap();
    assert!(hits.iter().any(|hit| hit.record.path.contains("发票")));
    assert!(hits.iter().any(|hit| hit.record.path.contains("فاتورة")));
    assert!(hits.iter().any(|hit| hit.record.path.contains("चालान")));
}
