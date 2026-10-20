use super::support::*;
use neutra_core::{BrowserIndex, DeltaChange, DeltaIndex, FileKind, Query, SortKey};

#[test]
fn transactional_live_updates_preserve_exact_rows_and_match_count() {
    let fixture = Fixture::build(
        "updates",
        &[
            record("/docs/alpha.txt", 1, 1, FileKind::File),
            record("/docs/beta.txt", 2, 2, FileKind::File),
            record("/docs/gamma.txt", 3, 3, FileKind::File),
        ],
    );
    let query = Query {
        terms: vec!["report".into()],
        sort: SortKey::NameAsc,
        limit: 10,
        ..Query::default()
    };
    fixture.prepared(&query, None, None);
    let mut delta = DeltaIndex::open(
        &fixture.path.with_extension("delta"),
        fixture.base.generation(),
    )
    .unwrap();
    let changes = [
        DeltaChange::Remove("/docs/alpha.txt".into()),
        DeltaChange::Upsert(record("/docs/report.txt", 8, 8, FileKind::File)),
        DeltaChange::Upsert(record("/docs/report-new.txt", 9, 9, FileKind::File)),
    ];
    delta.apply_batch(changes.clone()).unwrap();
    BrowserIndex::apply_changes(&fixture.path, fixture.base.generation(), &changes).unwrap();
    BrowserIndex::mark_delta(&fixture.path, &delta).unwrap();
    let reference = fixture.base.search_with_delta(&query, &delta).unwrap();
    let cached = fixture
        .browser
        .search_cached(&fixture.path, &query, None)
        .unwrap()
        .expect("maintained page stays warm");
    assert_eq!(paths(&cached.0), paths(&reference.0));
    assert_eq!(cached.1.matched, 2);
    assert_eq!(cached.1.matched, reference.1.matched);
    assert_eq!(cached.1.scanned, 0);
    let rename = [
        DeltaChange::Remove("/docs/report.txt".into()),
        DeltaChange::Upsert(record("/docs/renamed-report.txt", 11, 11, FileKind::File)),
    ];
    delta.apply_batch(rename.clone()).unwrap();
    BrowserIndex::apply_changes(&fixture.path, fixture.base.generation(), &rename).unwrap();
    BrowserIndex::mark_delta(&fixture.path, &delta).unwrap();
    let reference = fixture.base.search_with_delta(&query, &delta).unwrap();
    let cached = fixture
        .browser
        .search_cached(&fixture.path, &query, None)
        .unwrap()
        .expect("rename stays transactional");
    assert_eq!(paths(&cached.0), paths(&reference.0));
    assert_eq!(cached.1.matched, reference.1.matched);
    drop(delta);
    fixture.finish();
}

#[test]
fn subtree_removal_invalidates_every_cached_page_at_once() {
    let fixture = Fixture::build(
        "prefix",
        &[
            record("/docs/one.txt", 1, 1, FileKind::File),
            record("/docs/two.txt", 2, 2, FileKind::File),
            record("/other/one.txt", 3, 3, FileKind::File),
        ],
    );
    let cached_query = Query {
        terms: vec!["one".into()],
        sort: SortKey::PathAsc,
        limit: 10,
        ..Query::default()
    };
    let other_query = Query {
        terms: vec!["two".into()],
        sort: SortKey::PathAsc,
        limit: 10,
        ..Query::default()
    };
    fixture.prepared(&cached_query, None, None);
    fixture.prepared(&other_query, None, None);
    let mut delta = DeltaIndex::open(
        &fixture.path.with_extension("delta"),
        fixture.base.generation(),
    )
    .unwrap();
    let changes = [DeltaChange::RemovePrefix("/docs".into())];
    delta.apply_batch(changes.clone()).unwrap();
    BrowserIndex::apply_changes(&fixture.path, fixture.base.generation(), &changes).unwrap();
    BrowserIndex::mark_delta(&fixture.path, &delta).unwrap();
    assert!(fixture
        .browser
        .search_cached(&fixture.path, &cached_query, None)
        .unwrap()
        .is_none());
    assert!(fixture
        .browser
        .search_cached(&fixture.path, &other_query, None)
        .unwrap()
        .is_none());
    let (fallback, stats) = fixture.prepared(&cached_query, Some(&delta), None);
    let reference = fixture
        .base
        .search_with_delta(&cached_query, &delta)
        .unwrap();
    assert_eq!(paths(&fallback), paths(&reference.0));
    assert_eq!(stats.matched, reference.1.matched);
    drop(delta);
    fixture.finish();
}
