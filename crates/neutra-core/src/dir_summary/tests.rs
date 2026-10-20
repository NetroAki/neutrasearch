use crate::CompactIndex;
use crate::FsKind;

fn record(path: &str, size: u64, kind: FileKind) -> FileRecord {
    FileRecord {
        path: path.into(),
        size,
        mtime: 0,
        mode: 0,
        kind,
        fs: FsKind::Btrfs,
        native_id: size + 100,
        native_parent: 0,
        source: 0,
        disk: 0,
    }
}

fn cleanup(path: &Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(directory_summary_path(path));
}

#[test]
fn entry_decode_follows_the_file_version() {
    let current = DirectorySummaryEntry {
        source: 0,
        path: "/".into(),
        logical_bytes: 10,
        physical_bytes: 4,
        file_count: 1,
        directory_count: 0,
        children: Vec::new(),
    };
    let frame = bincode::serialize(&current).unwrap();
    assert_eq!(decode_entry(VERSION, &frame).unwrap().physical_bytes, 4);
    let legacy = OldSummaryEntry {
        source: 0,
        path: "/".into(),
        logical_bytes: 10,
        file_count: 1,
        directory_count: 0,
        children: Vec::new(),
    };
    let frame = bincode::serialize(&legacy).unwrap();
    let decoded = decode_entry(MIN_READABLE_VERSION, &frame).unwrap();
    assert_eq!(decoded.logical_bytes, 10);
    assert_eq!(decoded.physical_bytes, 0);
}

#[test]
fn aggregates_nested_logical_sizes_and_children() {
    let records = vec![
        record("/home/a/one.txt", 10, FileKind::File),
        record("/home/a/two.txt", 20, FileKind::File),
        record("/home/a/nested", 0, FileKind::Dir),
        record("/home/a/nested/three.txt", 30, FileKind::File),
    ];
    let summary = DirectorySummary::from_records(&records, 7).unwrap();
    let home = summary.get(0, "/home").unwrap();
    assert_eq!((home.logical_bytes, home.file_count), (60, 3));
    let folder = summary.get(0, "/home/a").unwrap();
    assert_eq!((folder.logical_bytes, folder.file_count), (60, 3));
    assert_eq!(folder.directory_count, 1);
    let nested = summary
        .children(0, "/home/a")
        .unwrap()
        .into_iter()
        .find(|child| child.path.as_ref() == "/home/a/nested")
        .unwrap();
    assert_eq!((nested.logical_bytes, nested.file_count), (30, 1));
}

#[test]
fn duplicate_source_path_contributes_once() {
    let records = vec![
        record("/same.txt", 10, FileKind::File),
        record("/same.txt", 99, FileKind::File),
    ];
    let summary = DirectorySummary::from_records(&records, 1).unwrap();
    assert_eq!(summary.get(0, "/").unwrap().logical_bytes, 10);
}

#[test]
fn sidecar_roundtrip_is_generation_bound() {
    let path = std::env::temp_dir().join(format!("neutra-dir-summary-{}.nsx", std::process::id()));
    cleanup(&path);
    let records = vec![record("/docs/readme.md", 42, FileKind::File)];
    DirectorySummary::build(&records, &path, 42).unwrap();
    let loaded = DirectorySummary::open_for_compact(&path, 42).unwrap();
    assert_eq!(loaded.get(0, "/docs").unwrap().logical_bytes, 42);
    assert!(DirectorySummary::open_for_compact(&path, 41).is_err());
    cleanup(&path);
}

#[test]
fn normalizes_windows_and_unc_paths() {
    let records = vec![
        record(r"C:\Users\Alex\report.txt", 4, FileKind::File),
        record(r"\\server\share\team\plan.txt", 5, FileKind::File),
    ];
    let summary = DirectorySummary::from_records(&records, 3).unwrap();
    assert!(summary.get(0, "C:/Users/Alex").is_some());
    assert!(summary.get(0, "//server/share/team").is_some());
}

#[test]
fn publish_moves_the_generation_bound_sidecar_with_the_base() {
    let base_path =
        std::env::temp_dir().join(format!("neutra-dir-publish-{}.nsx", std::process::id()));
    let staged_path =
        std::env::temp_dir().join(format!("neutra-dir-publish-{}.compact", std::process::id()));
    cleanup(&base_path);
    cleanup(&staged_path);
    let old_records = vec![record("/old.txt", 1, FileKind::File)];
    let new_records = vec![record("/new.txt", 2, FileKind::File)];
    CompactIndex::build_with_summary(&old_records, &base_path).unwrap();
    let staged = CompactIndex::build_with_summary(&new_records, &staged_path).unwrap();
    CompactIndex::publish(&staged_path, &base_path).unwrap();
    let summary = DirectorySummary::open_for_compact(&base_path, staged.generation).unwrap();
    assert!(summary
        .get(0, "/")
        .unwrap()
        .children
        .iter()
        .any(|child| child.path.as_ref() == "/new.txt"));
    assert!(!DirectorySummary::path_for(&staged_path).exists());
    cleanup(&base_path);
    cleanup(&staged_path);
}
use super::*;
