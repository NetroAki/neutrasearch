use super::support::*;
use neutra_core::{BrowserIndex, DeltaChange, DeltaIndex, FileKind, Query, SortKey};

#[test]
fn deletes_exhausting_the_retained_reserve_fall_back_to_the_engine() {
    let rows: Vec<_> = (0..4150)
        .map(|n| {
            record(
                &format!("/items/match-{n:04}.txt"),
                n,
                n as i64,
                FileKind::File,
            )
        })
        .collect();
    let fixture = Fixture::build("reserve", &rows);
    let query = Query {
        terms: vec!["match".into()],
        sort: SortKey::PathAsc,
        limit: 100,
        ..Query::default()
    };
    fixture.prepared(&query, None, None);
    let mut delta = DeltaIndex::open(
        &fixture.path.with_extension("delta"),
        fixture.base.generation(),
    )
    .unwrap();
    let removals: Vec<DeltaChange> = (0..4050)
        .map(|n| DeltaChange::Remove(format!("/items/match-{n:04}.txt").into()))
        .collect();
    delta.apply_batch(removals.clone()).unwrap();
    BrowserIndex::apply_changes(&fixture.path, fixture.base.generation(), &removals).unwrap();
    BrowserIndex::mark_delta(&fixture.path, &delta).unwrap();
    assert!(
        fixture
            .browser
            .search_cached(&fixture.path, &query, None)
            .unwrap()
            .is_none(),
        "exhausted reserve must not serve a short page"
    );
    let (fallback, stats) = fixture.prepared(&query, Some(&delta), None);
    let reference = fixture.base.search_with_delta(&query, &delta).unwrap();
    assert_eq!(paths(&fallback), paths(&reference.0));
    assert_eq!(stats.matched, 100);
    assert_eq!(stats.matched, reference.1.matched);
    drop(delta);
    fixture.finish();
}

#[test]
fn ninth_distinct_prepared_query_evicts_the_oldest_page() {
    let fixture = Fixture::build(
        "eviction",
        &[record("/evict/common.txt", 1, 1, FileKind::File)],
    );
    let queries: Vec<Query> = ALL_SORTS
        .iter()
        .map(|sort| Query {
            terms: vec!["common".into()],
            sort: *sort,
            limit: 10,
            ..Query::default()
        })
        .collect();
    for query in &queries {
        fixture.prepared(query, None, None);
    }
    assert!(
        fixture
            .browser
            .search_cached(&fixture.path, &queries[0], None)
            .unwrap()
            .is_none(),
        "capacity holds eight pages"
    );
    for query in &queries[1..] {
        let cached = fixture
            .browser
            .search_cached(&fixture.path, query, None)
            .unwrap()
            .expect("newest eight pages stay warm");
        assert_eq!(cached.1.scanned, 0);
    }
    fixture.finish();
}
