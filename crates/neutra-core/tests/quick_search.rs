use neutra_core::{
    BrowserIndex, CompactIndex, DeltaChange, DeltaIndex, FileKind, FileRecord, FsKind, MatchFields,
    Query, SpillAccumulator,
};

fn record(path: &str, mtime: i64) -> FileRecord {
    FileRecord {
        path: path.into(),
        size: 1,
        disk: 1,
        mtime,
        mode: 0,
        kind: FileKind::File,
        fs: FsKind::Btrfs,
        native_id: 0,
        native_parent: 0,
        source: 0,
    }
}

fn ranked(hits: &[neutra_core::SearchHit]) -> Vec<(u32, &str)> {
    hits.iter()
        .map(|hit| (hit.score, hit.record.path.as_ref()))
        .collect()
}

#[test]
fn quick_recall_matches_full_ranking_and_refuses_unproven_pages() {
    let root = std::env::temp_dir().join(format!("ns-quick-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("index.nsx");
    let rows = vec![
        record("/one/stems", 1),
        record("/two/STEMS", 2),
        record("/one/stems.wav", 3),
        record("/two/stems.wav", 4),
        record("/one/stems-old.wav", 5),
        record("/one/the-stems.wav", 100),
        record("/stems/other.wav", 200),
        record("/one/something.wav", 300),
    ];
    let mut spill = SpillAccumulator::begin(&path).unwrap();
    spill.push_batch(rows).unwrap();
    CompactIndex::rebuild_streamed(spill.finish().unwrap(), &path).unwrap();
    let base = CompactIndex::open_fast(&path).unwrap();
    BrowserIndex::ensure(&path, base.generation()).unwrap();
    let browser = BrowserIndex::open(&path, base.generation()).unwrap();
    for term in ["s", "st", "stems", "STEMS"] {
        for limit in [1, 2, 4, 5] {
            let query = Query {
                terms: vec![term.into()],
                match_fields: MatchFields::Name,
                limit,
                ..Query::default()
            };
            let hits = browser
                .search_quick(&path, &query, &|| false)
                .unwrap()
                .unwrap();
            assert_eq!(
                ranked(&hits),
                ranked(&base.search(&query).unwrap().0),
                "{term} / {limit}"
            );
        }
    }
    let mut query = Query {
        terms: vec!["stems".into()],
        match_fields: MatchFields::Name,
        limit: 6,
        ..Query::default()
    };
    assert!(browser
        .search_quick(&path, &query, &|| false)
        .unwrap()
        .is_none());
    query.limit = 2;
    for term in ["k", "i", "café", "no-such-file"] {
        query.terms = vec![term.into()];
        assert!(browser
            .search_quick(&path, &query, &|| false)
            .unwrap()
            .is_none());
    }
    query.terms = vec!["stems".into()];
    query.case_sensitive = true;
    assert!(browser
        .search_quick(&path, &query, &|| false)
        .unwrap()
        .is_none());
    query.case_sensitive = false;
    assert_eq!(
        browser
            .search_quick(&path, &query, &|| true)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::Interrupted
    );

    check_live_changes(&path, &base, &browser, &query);
    drop(browser);
    drop(base);
    std::fs::remove_dir_all(root).unwrap();
}

fn check_live_changes(
    path: &std::path::Path,
    base: &CompactIndex,
    browser: &BrowserIndex,
    query: &Query,
) {
    let mut delta = DeltaIndex::open(&path.with_extension("delta"), base.generation()).unwrap();
    BrowserIndex::mark_delta(&path, &delta).unwrap();
    let edit = DeltaChange::Upsert(record("/new/stems", 500));
    delta.apply(edit.clone()).unwrap();
    delta.sync().unwrap();
    assert!(browser
        .search_quick(&path, &query, &|| false)
        .unwrap()
        .is_none());
    let hits = browser
        .search_quick_with_delta(&path, &query, Some(&mut delta), &|| false)
        .unwrap()
        .unwrap();
    assert_eq!(
        ranked(&hits),
        ranked(&base.search_with_delta(&query, &delta).unwrap().0)
    );
    BrowserIndex::apply_changes(&path, base.generation(), &[edit]).unwrap();
    BrowserIndex::mark_delta(&path, &delta).unwrap();
    let hits = browser
        .search_quick(&path, &query, &|| false)
        .unwrap()
        .unwrap();
    assert_eq!(
        ranked(&hits),
        ranked(&base.search_with_delta(&query, &delta).unwrap().0)
    );
    assert_eq!(hits[0].record.path.as_ref(), "/new/stems");
    let changes = [
        DeltaChange::Remove("/new/stems".into()),
        DeltaChange::RemovePrefix("/two".into()),
        DeltaChange::Upsert(record("/one/stems.wav", 1000)),
    ];
    delta.apply_batch(changes).unwrap();
    delta.sync().unwrap();
    // The catalog still holds a removed best match and the old modification
    // time. The merged page must agree with the authoritative base plus WAL.
    let hits = browser
        .search_quick_with_delta(&path, &query, Some(&mut delta), &|| false)
        .unwrap()
        .unwrap();
    assert_eq!(
        ranked(&hits),
        ranked(&base.search_with_delta(&query, &delta).unwrap().0)
    );
}

#[test]
fn common_exact_names_use_persistent_order_and_merge_live_edits() {
    let root = std::env::temp_dir().join(format!("ns-quick-common-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("index.nsx");
    let mut spill = SpillAccumulator::begin(&path).unwrap();
    spill
        .push_batch(
            (0..5000)
                .map(|n| record(&format!("/folder-{n:05}/a"), n))
                .collect(),
        )
        .unwrap();
    CompactIndex::rebuild_streamed(spill.finish().unwrap(), &path).unwrap();
    let base = CompactIndex::open_fast(&path).unwrap();
    BrowserIndex::ensure(&path, base.generation()).unwrap();
    let mut delta = DeltaIndex::open(&path.with_extension("delta"), base.generation()).unwrap();
    BrowserIndex::mark_delta(&path, &delta).unwrap();
    let browser = BrowserIndex::open(&path, base.generation()).unwrap();
    let query = Query {
        terms: vec!["a".into()],
        match_fields: MatchFields::Name,
        limit: 8,
        ..Query::default()
    };
    let hits = browser
        .search_quick(&path, &query, &|| false)
        .unwrap()
        .unwrap();
    assert_eq!(ranked(&hits), ranked(&base.search(&query).unwrap().0));

    delta
        .apply_batch([
            DeltaChange::Remove("/folder-04999/a".into()),
            DeltaChange::Upsert(record("/folder-00000/a", 10000)),
        ])
        .unwrap();
    delta.sync().unwrap();
    let hits = browser
        .search_quick_with_delta(&path, &query, Some(&mut delta), &|| false)
        .unwrap()
        .unwrap();
    assert_eq!(
        ranked(&hits),
        ranked(&base.search_with_delta(&query, &delta).unwrap().0)
    );
    drop(delta);
    drop(browser);
    drop(base);
    std::fs::remove_dir_all(root).unwrap();
}
