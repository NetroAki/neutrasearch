use super::*;
use crate::{DeltaChange, FileKind, FsKind, SortKey};

fn record(n: u64) -> FileRecord {
    FileRecord {
        path: format!("/library/{n}/call track{}.wav", n % 7).into(),
        size: n % 5,
        mtime: (n % 3) as i64,
        mode: 0,
        kind: FileKind::File,
        fs: FsKind::Btrfs,
        native_id: n,
        native_parent: 0,
        source: 0,
        disk: 0,
    }
}

#[test]
fn cursor_pages_match_complete_order_with_ties_and_live_delta() {
    let dir = std::env::temp_dir().join(format!("neutra-cursor-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("index.nsx");
    let records: Vec<_> = (0..137).map(record).collect();
    CompactIndex::build(&records, &path).unwrap();
    let index = CompactIndex::open(&path).unwrap();
    let mut delta = DeltaIndex::open(&path.with_extension("delta"), index.generation()).unwrap();
    delta
        .apply(DeltaChange::Remove(records[0].path.clone()))
        .unwrap();
    let mut updated = record(1);
    updated.mtime = 10;
    delta.apply(DeltaChange::Upsert(updated)).unwrap();
    delta.apply(DeltaChange::Upsert(record(999))).unwrap();
    for sort in [
        SortKey::Relevance,
        SortKey::NameAsc,
        SortKey::NameDesc,
        SortKey::PathAsc,
        SortKey::PathDesc,
        SortKey::SizeAsc,
        SortKey::SizeDesc,
        SortKey::MtimeAsc,
        SortKey::MtimeDesc,
    ] {
        for regex in [None, Some("call.*".into())] {
            assert_pages(&index, &delta, sort, regex);
        }
    }
    assert_eq!(
        index
            .search_after_interruptible(&Query::parse("call"), Some(&delta), None, &|| true)
            .unwrap_err()
            .kind(),
        io::ErrorKind::Interrupted
    );
    drop(delta);
    drop(index);
    std::fs::remove_dir_all(dir).unwrap();
}

fn assert_pages(index: &CompactIndex, delta: &DeltaIndex, sort: SortKey, regex: Option<String>) {
    let mut query = Query::parse("call");
    query.sort = sort;
    query.regex = regex;
    query.limit = 0;
    let (complete, stats) = index.search_with_delta(&query, delta).unwrap();
    assert_eq!(stats.matched, 137);
    query.limit = 7;
    let mut actual = Vec::new();
    let mut cursor = None;
    for _ in 0..21 {
        let (page, counts) = index
            .search_after_interruptible(&query, Some(delta), cursor.as_ref(), &|| false)
            .unwrap();
        assert_eq!(counts.matched, stats.matched);
        assert!(page.len() <= 7);
        if page.is_empty() {
            break;
        }
        cursor = page.last().map(|hit| hit.record.clone());
        actual.extend(page.into_iter().map(|hit| (hit.score, hit.record.path)));
    }
    let expected: Vec<_> = complete
        .into_iter()
        .map(|hit| (hit.score, hit.record.path))
        .collect();
    assert_eq!(actual, expected, "cursor order disagrees for {sort:?}");
}
