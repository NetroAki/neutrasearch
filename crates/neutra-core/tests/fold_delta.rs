//! The fold path exists on Linux only.
#![cfg(target_os = "linux")]

//! A fold must be indistinguishable from rebuilding the merged records: same
//! records, same search answers, same directory totals.
use neutra_core::{
    CompactIndex, DeltaChange, DeltaIndex, DirectorySummary, FileKind, FileRecord, FsKind, Query,
    SpillAccumulator,
};
use std::collections::BTreeMap;

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
fn fold_matches_a_full_rebuild() {
    let root = std::env::temp_dir().join(format!("neutra-fold-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir(&root).unwrap();
    let base_path = root.join("base.nsx");
    let mut logical: BTreeMap<String, FileRecord> = BTreeMap::new();
    let mut records = vec![record("/data", 0, FileKind::Dir)];
    for dir in 0..12 {
        records.push(record(&format!("/data/d{dir:02}"), 0, FileKind::Dir));
        for file in 0..40 {
            records.push(record(
                &format!("/data/d{dir:02}/file{file:02}.txt"),
                (dir * 100 + file) as u64 + 1,
                FileKind::File,
            ));
        }
    }
    for r in &records {
        logical.insert(r.path.to_string(), r.clone());
    }
    let base = build(&base_path, records);
    let mut delta = DeltaIndex::open(&root.join("base.delta"), base.generation()).unwrap();
    let mut changes = vec![
        // New file before everything, between blocks, inside a block, and at the end.
        DeltaChange::Upsert(record("/aaa/first.txt", 7, FileKind::File)),
        DeltaChange::Upsert(record("/data/d03/file10x.txt", 8, FileKind::File)),
        DeltaChange::Upsert(record("/data/d05/zzz-new.txt", 9, FileKind::File)),
        DeltaChange::Upsert(record("/zzz/last.txt", 10, FileKind::File)),
        // Changed file and removed files.
        DeltaChange::Upsert(record("/data/d07/file05.txt", 5000, FileKind::File)),
        DeltaChange::Remove("/data/d08/file00.txt".into()),
        DeltaChange::Remove("/data/d08/file01.txt".into()),
        // A whole directory removed, then one file recreated in it.
        DeltaChange::RemovePrefix("/data/d10".into()),
        DeltaChange::Upsert(record("/data/d10/back.txt", 11, FileKind::File)),
    ];
    for file in 0..40 {
        changes.push(DeltaChange::Remove(
            format!("/data/d01/file{file:02}.txt").into(),
        ));
    }
    delta.apply_batch(changes.clone()).unwrap();
    delta.sync().unwrap();
    for change in changes {
        match change {
            DeltaChange::Upsert(r) => {
                logical.insert(r.path.to_string(), r);
            }
            DeltaChange::Remove(p) => {
                logical.remove(&*p);
            }
            DeltaChange::RemovePrefix(p) => {
                let prefix = format!("{p}/");
                logical.retain(|k, _| k != &*p && !k.starts_with(&prefix));
            }
        }
    }

    let staged = root.join("base.nsx.compact");
    CompactIndex::fold_delta(&base, &delta, &staged).unwrap();
    let folded = CompactIndex::open(&staged).unwrap();
    let rebuilt = build(
        &root.join("rebuilt.nsx"),
        logical.values().cloned().collect(),
    );

    assert_eq!(paths(&folded), paths(&rebuilt));
    assert_eq!(folded.len(), rebuilt.len());
    for term in [
        "file05", "txt", "zzz", "first", "d08", "back", "file10x", "data/d01",
    ] {
        let query = Query::parse(term);
        let (mut a, _) = folded.search(&query).unwrap();
        let (mut b, _) = rebuilt.search(&query).unwrap();
        a.sort_by(|x, y| x.record.path.cmp(&y.record.path));
        b.sort_by(|x, y| x.record.path.cmp(&y.record.path));
        let left: Vec<_> = a
            .iter()
            .map(|h| (h.record.path.clone(), h.record.size))
            .collect();
        let right: Vec<_> = b
            .iter()
            .map(|h| (h.record.path.clone(), h.record.size))
            .collect();
        assert_eq!(left, right, "search {term}");
    }
    let totals = DirectorySummary::open_for_compact(&staged, folded.generation()).unwrap();
    let expected =
        DirectorySummary::open_for_compact(&root.join("rebuilt.nsx"), rebuilt.generation())
            .unwrap();
    for dir in ["/data", "/data/d01", "/data/d07", "/data/d10", "/data/d03"] {
        let (a, b) = (totals.get(0, dir), expected.get(0, dir));
        assert_eq!(
            a.map(|e| (e.logical_bytes, e.file_count)),
            b.map(|e| (e.logical_bytes, e.file_count)),
            "summary {dir}"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}
