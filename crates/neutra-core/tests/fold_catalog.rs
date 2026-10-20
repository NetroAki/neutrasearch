//! The fold path exists on Linux only.
#![cfg(target_os = "linux")]

//! The browse catalog must address records exactly over a folded base, whose
//! compaction can leave short blocks in the middle of the file.
use neutra_core::{
    CompactIndex, DeltaChange, DeltaIndex, FileKind, FileRecord, FsKind, Query, SpillAccumulator,
};

fn record(path: &str, size: u64, kind: FileKind) -> FileRecord {
    FileRecord {
        path: path.into(),
        size,
        disk: size,
        mtime: size as i64,
        mode: 0,
        kind,
        fs: FsKind::Btrfs,
        native_id: size + 100,
        native_parent: 100,
        source: 0,
    }
}

fn build(path: &std::path::Path, records: Vec<FileRecord>) -> CompactIndex {
    let mut spill = SpillAccumulator::begin(path).unwrap();
    spill.push_batch(records).unwrap();
    CompactIndex::rebuild_streamed(spill.finish().unwrap(), path).unwrap();
    CompactIndex::open(path).unwrap()
}

fn paths(index: &CompactIndex) -> Vec<String> {
    let mut all: Vec<String> = index
        .records()
        .unwrap()
        .into_iter()
        .map(|r| r.path.to_string())
        .collect();
    all.sort();
    all
}

#[cfg(unix)]
#[test]
fn catalog_ids_stay_exact_over_a_folded_base_with_short_blocks() {
    use neutra_core::{BrowserIndex, SortKey};
    let root = std::env::temp_dir().join(format!("neutra-fold-cat-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir(&root).unwrap();
    let base_path = root.join("base.nsx");
    let mut records = vec![record("/data", 0, FileKind::Dir)];
    for dir in 0..12 {
        for file in 0..40 {
            records.push(record(
                &format!("/data/d{dir:02}/file{file:02}.txt"),
                (dir * 100 + file) as u64 + 1,
                FileKind::File,
            ));
        }
    }
    let base = build(&base_path, records);
    let mut delta = DeltaIndex::open(&root.join("base.delta"), base.generation()).unwrap();
    // Inserting inside untouched runs makes the fold emit short blocks mid-file.
    let inserts: Vec<_> = (0..12)
        .map(|dir| {
            DeltaChange::Upsert(record(
                &format!("/data/d{dir:02}/file05x.txt"),
                9,
                FileKind::File,
            ))
        })
        .collect();
    delta.apply_batch(inserts).unwrap();
    delta.sync().unwrap();
    let staged = root.join("folded.nsx");
    CompactIndex::fold_delta(&base, &delta, &staged).unwrap();
    let folded = CompactIndex::open(&staged).unwrap();
    BrowserIndex::ensure(&staged, folded.generation()).unwrap();
    let changes = [
        DeltaChange::Remove("/data/d11/file39.txt".into()),
        DeltaChange::Upsert(record("/data/d11/file38.txt", 77, FileKind::File)),
        DeltaChange::RemovePrefix("/data/d10".into()),
    ];
    BrowserIndex::apply_changes(&staged, folded.generation(), &changes).unwrap();
    let mut expected = paths(&folded);
    expected.retain(|p| p != "/data/d11/file39.txt" && !p.starts_with("/data/d10/"));
    let browser = BrowserIndex::open(&staged, folded.generation()).unwrap();
    let query = Query {
        sort: SortKey::PathAsc,
        limit: 2000,
        ..Query::default()
    };
    let (hits, _) = browser.search_page(&folded, &query, None, 0).unwrap();
    let actual: Vec<String> = hits.iter().map(|h| h.record.path.to_string()).collect();
    assert_eq!(actual, expected);
    let changed = hits
        .iter()
        .find(|h| &*h.record.path == "/data/d11/file38.txt")
        .unwrap();
    assert_eq!(changed.record.size, 77);
    drop(browser);
    let _ = std::fs::remove_dir_all(&root);
}
