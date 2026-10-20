use super::*;
use crate::dir_summary::DirectorySummary;
use crate::{CompactIndex, DeltaChange, DeltaIndex, FileKind};

fn record(path: &str, size: u64, kind: FileKind) -> FileRecord {
    FileRecord {
        path: path.into(),
        size,
        mtime: 0,
        mode: 0,
        kind,
        fs: crate::FsKind::Btrfs,
        native_id: size + 100,
        native_parent: 0,
        source: 0,
        disk: 0,
    }
}

fn cleanup(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(crate::dir_summary::DirectorySummary::path_for(path));
}

fn remove_delta(path: &std::path::Path) {
    let mut delta_path = path.to_path_buf();
    delta_path.set_extension("delta");
    let _ = std::fs::remove_file(&delta_path);
    let mut lock = delta_path.as_os_str().to_os_string();
    lock.push(".lock");
    let _ = std::fs::remove_file(std::path::PathBuf::from(lock));
}

#[test]
fn overlay_removes_all_sources_for_a_path_key() {
    let path = std::env::temp_dir().join(format!("neutra-dir-sources-{}.nsx", std::process::id()));
    cleanup(&path);
    let mut first = record("/shared.txt", 10, FileKind::File);
    first.source = 1;
    let mut second = record("/shared.txt", 20, FileKind::File);
    second.source = 2;
    let records = vec![first, second];
    CompactIndex::build_with_summary(&records, &path).unwrap();
    let compact = CompactIndex::open(&path).unwrap();
    let summary = DirectorySummary::open_for_compact(&path, compact.generation()).unwrap();
    let mut delta_path = path.clone();
    delta_path.set_extension("delta");
    let mut delta = DeltaIndex::open(&delta_path, compact.generation()).unwrap();
    delta
        .apply(DeltaChange::Remove("/shared.txt".into()))
        .unwrap();
    delta.sync().unwrap();
    let overlay = DirectorySummaryOverlay::from_delta(summary, &compact, &delta).unwrap();
    assert_eq!(overlay.get(1, "/").unwrap().unwrap().logical_bytes, 0);
    assert_eq!(overlay.get(2, "/").unwrap().unwrap().logical_bytes, 0);
    drop(delta);
    drop(compact);
    cleanup(&path);
    remove_delta(&path);
}

#[test]
fn overlay_removal_normalizes_separator_variants() {
    let path =
        std::env::temp_dir().join(format!("neutra-dir-separators-{}.nsx", std::process::id()));
    cleanup(&path);
    let records = vec![record(r"C:\Data\report.txt", 12, FileKind::File)];
    CompactIndex::build_with_summary(&records, &path).unwrap();
    let compact = CompactIndex::open(&path).unwrap();
    assert!(compact
        .record_by_path(0, "C:/Data/report.txt")
        .unwrap()
        .is_some());
    let summary = DirectorySummary::open_for_compact(&path, compact.generation()).unwrap();
    let mut delta_path = path.clone();
    delta_path.set_extension("delta");
    let mut delta = DeltaIndex::open(&delta_path, compact.generation()).unwrap();
    delta
        .apply(DeltaChange::Remove("C:/Data/report.txt".into()))
        .unwrap();
    delta.sync().unwrap();
    let overlay = DirectorySummaryOverlay::from_delta(summary, &compact, &delta).unwrap();
    assert_eq!(overlay.get(0, "/").unwrap().unwrap().logical_bytes, 0);
    drop(delta);
    drop(compact);
    cleanup(&path);
    remove_delta(&path);
}

#[test]
fn overlay_updates_ancestor_totals_and_direct_children() {
    let path = std::env::temp_dir().join(format!("neutra-dir-overlay-{}.nsx", std::process::id()));
    cleanup(&path);
    let records = vec![
        record("/docs", 0, FileKind::Dir),
        record("/docs/a.txt", 10, FileKind::File),
        record("/docs/sub", 0, FileKind::Dir),
        record("/docs/sub/b.txt", 20, FileKind::File),
    ];
    CompactIndex::build_with_summary(&records, &path).unwrap();
    let compact = CompactIndex::open(&path).unwrap();
    let summary = DirectorySummary::open_for_compact(&path, compact.generation()).unwrap();
    let mut delta_path = path.clone();
    delta_path.set_extension("delta");
    let mut delta = DeltaIndex::open(&delta_path, compact.generation()).unwrap();
    delta
        .apply(DeltaChange::Upsert(record(
            "/docs/a.txt",
            30,
            FileKind::File,
        )))
        .unwrap();
    delta
        .apply(DeltaChange::Remove("/docs/sub/b.txt".into()))
        .unwrap();
    delta.sync().unwrap();
    let overlay = DirectorySummaryOverlay::from_delta(summary, &compact, &delta).unwrap();
    let docs = overlay.get(0, "/docs").unwrap().unwrap();
    assert_eq!(docs.logical_bytes, 30);
    let children = overlay.children(0, "/docs").unwrap();
    let file = children
        .iter()
        .find(|child| child.path.as_ref() == "/docs/a.txt")
        .unwrap();
    assert_eq!(file.logical_bytes, 30);
    let sub = children
        .iter()
        .find(|child| child.path.as_ref() == "/docs/sub")
        .unwrap();
    assert_eq!(sub.logical_bytes, 0);
    drop(delta);
    drop(compact);
    cleanup(&path);
    remove_delta(&path);
}

#[test]
fn adjusted_entries_project_base_plus_delta() {
    let path = std::env::temp_dir().join(format!("neutra-dir-adjusted-{}.nsx", std::process::id()));
    cleanup(&path);
    let records = vec![
        record("/docs", 0, FileKind::Dir),
        record("/docs/a.txt", 10, FileKind::File),
    ];
    CompactIndex::build_with_summary(&records, &path).unwrap();
    let compact = CompactIndex::open(&path).unwrap();
    let summary = DirectorySummary::open_for_compact(&path, compact.generation()).unwrap();
    let mut delta_path = path.clone();
    delta_path.set_extension("delta");
    let mut delta = DeltaIndex::open(&delta_path, compact.generation()).unwrap();
    delta
        .apply(DeltaChange::Upsert(record(
            "/docs/b.txt",
            5,
            FileKind::File,
        )))
        .unwrap();
    delta.sync().unwrap();
    let overlay = DirectorySummaryOverlay::from_delta(summary, &compact, &delta).unwrap();
    let docs = overlay
        .adjusted_entries()
        .unwrap()
        .into_iter()
        .find(|entry| entry.path.as_ref() == "/docs")
        .unwrap();
    assert_eq!(docs.logical_bytes, 15);
    assert_eq!(docs.file_count, 2);
    drop(delta);
    drop(compact);
    cleanup(&path);
    remove_delta(&path);
}
