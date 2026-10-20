use neutra_core::{
    CompactIndex, DeltaChange, DeltaIndex, FileKind, FileRecord, FsKind, Query, SortKey,
    SpillAccumulator,
};

fn record(n: u64) -> FileRecord {
    let folder = if n.is_multiple_of(4) {
        "removed"
    } else {
        "kept"
    };
    FileRecord {
        path: format!("/library/{folder}/track{n:05}.wav").into(),
        size: n % 31,
        disk: n % 31,
        mtime: (n % 17) as i64,
        mode: 0,
        kind: FileKind::File,
        fs: FsKind::Btrfs,
        native_id: n,
        native_parent: 0,
        source: 0,
    }
}

#[test]
fn batches_keep_exact_matches_order_and_cursor_with_prefixes_and_replacements() {
    let root = std::env::temp_dir().join(format!("neutra-search-batches-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let records: Vec<_> = (0..6001).map(record).collect();
    let path = root.join("base.nsx");
    build(records.clone(), &path);
    let base = CompactIndex::open(&path).unwrap();
    let mut delta = DeltaIndex::open(&path.with_extension("delta"), base.generation()).unwrap();
    delta
        .apply(DeltaChange::RemovePrefix("/library/removed".into()))
        .unwrap();
    delta.apply(DeltaChange::Remove(record(1).path)).unwrap();
    let mut replaced = record(2);
    replaced.size = 200;
    replaced.disk = 200;
    delta.apply(DeltaChange::Upsert(replaced.clone())).unwrap();
    let recreated = record(0);
    delta.apply(DeltaChange::Upsert(recreated.clone())).unwrap();
    let mut logical: Vec<_> = records
        .into_iter()
        .filter(|r| !r.native_id.is_multiple_of(4) && r.native_id > 2)
        .collect();
    logical.extend([replaced, recreated]);
    let expected_path = root.join("logical.nsx");
    build(logical.clone(), &expected_path);
    let expected = CompactIndex::open(&expected_path).unwrap();
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
        let mut query = Query::parse("a");
        query.sort = sort;
        query.limit = 73;
        let mut cursor = None;
        for _ in 0..3 {
            let (actual, stats) = base
                .search_after_interruptible(&query, Some(&delta), cursor.as_ref(), &|| false)
                .unwrap();
            let (want, counts) = expected
                .search_after_interruptible(&query, None, cursor.as_ref(), &|| false)
                .unwrap();
            assert_eq!(stats.matched, logical.len() as u64);
            assert_eq!(stats.matched, counts.matched);
            let paths = |hits: &[neutra_core::SearchHit]| {
                hits.iter()
                    .map(|h| (h.score, h.record.path.clone(), h.record.disk))
                    .collect::<Vec<_>>()
            };
            assert_eq!(paths(&actual), paths(&want), "order differs for {sort:?}");
            cursor = actual.last().map(|h| h.record.clone());
        }
    }
    drop(delta);
    drop(base);
    drop(expected);
    std::fs::remove_dir_all(root).unwrap();
}

fn build(records: Vec<FileRecord>, path: &std::path::Path) {
    let mut spill = SpillAccumulator::begin(path).unwrap();
    spill.push_batch(records).unwrap();
    CompactIndex::rebuild_streamed(spill.finish().unwrap(), path).unwrap();
}
