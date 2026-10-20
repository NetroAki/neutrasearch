use super::support::*;
use neutra_core::{
    BrowserIndex, CompactIndex, DeltaChange, DeltaIndex, FileKind, MatchFields, Query,
};
use std::sync::{Arc, Barrier};

#[test]
fn cached_search_writers_and_live_updates_coexist_with_a_wal_reader() {
    let records: Vec<_> = (0..80)
        .map(|n| {
            record(
                &format!("/docs/report-{n:03}.txt"),
                n,
                n as i64,
                FileKind::File,
            )
        })
        .collect();
    let fixture = Fixture::build("contention", &records);
    let query = Query {
        terms: vec!["report".into()],
        match_fields: MatchFields::Name,
        limit: 8,
        ..Query::default()
    };
    fixture.prepared(&query, None, None);

    let reader = rusqlite::Connection::open_with_flags(
        BrowserIndex::path_for(&fixture.path),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    reader
        .execute_batch("BEGIN; SELECT generation FROM metadata;")
        .unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let worker_path = fixture.path.clone();
    let worker_barrier = Arc::clone(&barrier);
    let worker = std::thread::spawn(move || {
        let base = CompactIndex::open_fast(&worker_path).unwrap();
        let browser = BrowserIndex::open(&worker_path, base.generation()).unwrap();
        worker_barrier.wait();
        for n in 0..32 {
            let query = Query {
                terms: vec![format!("report-{n:03}")],
                match_fields: MatchFields::Name,
                limit: 8,
                ..Query::default()
            };
            browser
                .search_prepared(&worker_path, &base, &query, None, None, &|| false)
                .unwrap();
        }
    });
    let mut delta = DeltaIndex::open(
        &fixture.path.with_extension("delta"),
        fixture.base.generation(),
    )
    .unwrap();
    BrowserIndex::mark_delta(&fixture.path, &delta).unwrap();
    barrier.wait();
    for n in 0..32 {
        let changes = [DeltaChange::Upsert(record(
            "/docs/report-live.txt",
            n,
            n as i64,
            FileKind::File,
        ))];
        delta.apply_batch(changes.clone()).unwrap();
        delta.sync().unwrap();
        BrowserIndex::apply_changes(&fixture.path, fixture.base.generation(), &changes).unwrap();
        BrowserIndex::mark_delta(&fixture.path, &delta).unwrap();
    }
    worker.join().unwrap();
    reader.execute_batch("ROLLBACK").unwrap();
    drop(reader);
    assert!(fixture.browser.covers_delta(&fixture.path).unwrap());
    assert_eq!(
        fixture
            .browser
            .record_by_path("/docs/report-live.txt")
            .unwrap()
            .unwrap()
            .size,
        31
    );
    drop(delta);
    fixture.finish();
}
