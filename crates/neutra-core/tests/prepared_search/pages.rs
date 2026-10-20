use super::support::*;
use neutra_core::{BrowserIndex, DeltaChange, DeltaIndex, FileKind, FileRecord, Query, SortKey};

#[test]
fn deep_cursor_page_falls_back_once_past_the_retained_frontier() {
    let rows: Vec<_> = (0..4100)
        .map(|n| {
            record(
                &format!("/deep/match-{n:04}.txt"),
                n,
                n as i64,
                FileKind::File,
            )
        })
        .collect();
    let fixture = Fixture::build("frontier", &rows);
    let query = Query {
        terms: vec!["match".into()],
        sort: SortKey::PathAsc,
        limit: 100,
        ..Query::default()
    };
    let expected = fixture
        .base
        .search(&Query {
            limit: 5000,
            ..query.clone()
        })
        .unwrap()
        .0;
    fixture.prepared(&query, None, None);
    assert!(fixture
        .browser
        .search_cached(&fixture.path, &query, Some(&expected[3999].record))
        .unwrap()
        .is_none());
    let (fallback, stats) = fixture.prepared(&query, None, Some(&expected[3999].record));
    assert_eq!(paths(&fallback), paths(&expected[4000..4100]));
    assert_eq!(stats.matched, 4100);
    fixture.finish();
}

#[test]
fn prepared_cursor_pages_are_exact_and_stale_wal_rejects_the_cache() {
    let rows: Vec<_> = (0..35)
        .map(|n| {
            record(
                &format!("/page/item-{n:02}.log"),
                n,
                n as i64,
                FileKind::File,
            )
        })
        .collect();
    let fixture = Fixture::build("cursor-wal", &rows);
    let query = Query {
        terms: vec!["item".into()],
        sort: SortKey::NameAsc,
        limit: 7,
        ..Query::default()
    };
    let expected = fixture
        .base
        .search(&Query {
            limit: 100,
            ..query.clone()
        })
        .unwrap()
        .0;
    fixture.prepared(&query, None, None);
    let mut after: Option<FileRecord> = None;
    let mut observed = Vec::new();
    loop {
        let page = fixture
            .browser
            .search_cached(&fixture.path, &query, after.as_ref())
            .unwrap()
            .expect("retained cursor page stays warm")
            .0;
        if page.is_empty() {
            break;
        }
        after = page.last().map(|hit| hit.record.clone());
        observed.extend(paths(&page));
    }
    assert_eq!(observed, paths(&expected));
    let mut delta = DeltaIndex::open(
        &fixture.path.with_extension("delta"),
        fixture.base.generation(),
    )
    .unwrap();
    delta
        .apply(DeltaChange::Upsert(record(
            "/page/item-new.log",
            99,
            99,
            FileKind::File,
        )))
        .unwrap();
    assert!(
        fixture
            .browser
            .search_cached(&fixture.path, &query, None)
            .unwrap()
            .is_none(),
        "uncovered WAL must reject the cache"
    );
    BrowserIndex::apply_changes(
        &fixture.path,
        fixture.base.generation(),
        &[DeltaChange::Upsert(record(
            "/page/item-new.log",
            99,
            99,
            FileKind::File,
        ))],
    )
    .unwrap();
    BrowserIndex::mark_delta(&fixture.path, &delta).unwrap();
    assert!(fixture
        .browser
        .search_cached(&fixture.path, &query, None)
        .unwrap()
        .is_some());
    drop(delta);
    fixture.finish();
}

#[test]
fn cancelled_preparation_returns_interrupted_without_publishing_a_page() {
    let fixture = Fixture::build(
        "cancel",
        &[record("/cancel/target.txt", 1, 1, FileKind::File)],
    );
    let query = Query {
        terms: vec!["target".into()],
        ..Query::default()
    };
    let error = fixture
        .browser
        .search_prepared(&fixture.path, &fixture.base, &query, None, None, &|| true)
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
    assert!(fixture
        .browser
        .search_cached(&fixture.path, &query, None)
        .unwrap()
        .is_none());
    fixture.finish();
}
