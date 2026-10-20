use super::*;
use crate::{DeltaChange, FileKind, FsKind, MatchFields};

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

fn fixture(label: &str) -> (std::path::PathBuf, CompactIndex, BrowserIndex, Query) {
    let path =
        std::env::temp_dir().join(format!("ns-page-race-{label}-{}.nsx", std::process::id()));
    cleanup(&path);
    CompactIndex::build(&[record("/a", 1), record("/folder/a.txt", 2)], &path).unwrap();
    let base = CompactIndex::open_fast(&path).unwrap();
    BrowserIndex::ensure(&path, base.generation()).unwrap();
    let browser = BrowserIndex::open(&path, base.generation()).unwrap();
    let mut query = Query::parse("a");
    query.match_fields = MatchFields::Name;
    query.limit = 100;
    (path, base, browser, query)
}

fn cleanup(path: &Path) {
    for suffix in [
        "",
        ".browse",
        ".browse.base",
        ".browse.lock",
        ".browse-wal",
        ".browse-shm",
        ".stale",
    ] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    for suffix in ["delta", "delta.lock"] {
        let _ = std::fs::remove_file(path.with_extension(suffix));
    }
}

#[test]
fn preparation_replays_live_edits_before_publication() {
    let (path, base, browser, query) = fixture("edits");
    let token = browser.preparation_token(&path, None).unwrap().unwrap();
    let (hits, stats) = base.search(&query).unwrap();
    let mut delta = DeltaIndex::open(&path.with_extension("delta"), base.generation()).unwrap();
    let changes = [
        DeltaChange::Remove("/a".into()),
        DeltaChange::Upsert(record("/new/a.wav", 9)),
        DeltaChange::Upsert(record("/folder/a.txt", 10)),
    ];
    delta.apply_batch(changes.to_vec()).unwrap();
    delta.sync().unwrap();
    BrowserIndex::apply_changes(&path, base.generation(), &changes).unwrap();
    BrowserIndex::mark_delta(&path, &delta).unwrap();
    browser
        .save_page(&path, &query, token, &hits, stats.matched)
        .unwrap();
    let (cached, cached_stats) = browser.search_cached(&path, &query, None).unwrap().unwrap();
    let (expected, expected_stats) = base.search_with_delta(&query, &delta).unwrap();
    assert_eq!(cached_stats.matched, expected_stats.matched);
    assert_eq!(
        cached
            .iter()
            .map(|h| (&h.record.path, h.record.mtime))
            .collect::<Vec<_>>(),
        expected
            .iter()
            .map(|h| (&h.record.path, h.record.mtime))
            .collect::<Vec<_>>()
    );
    drop((browser, base, delta));
    cleanup(&path);
}

#[test]
fn subtree_edit_during_preparation_discards_the_page() {
    let (path, base, browser, query) = fixture("prefix");
    let token = browser.preparation_token(&path, None).unwrap().unwrap();
    let (hits, stats) = base.search(&query).unwrap();
    BrowserIndex::apply_changes(
        &path,
        base.generation(),
        &[DeltaChange::RemovePrefix("/folder".into())],
    )
    .unwrap();
    browser
        .save_page(&path, &query, token, &hits, stats.matched)
        .unwrap();
    assert!(browser
        .search_cached(&path, &query, None)
        .unwrap()
        .is_none());
    drop((browser, base));
    cleanup(&path);
}

#[test]
fn a_stale_marker_rejects_previously_prepared_rows() {
    let (path, base, browser, query) = fixture("stale");
    browser
        .search_prepared(&path, &base, &query, None, None, &|| false)
        .unwrap();
    assert!(browser
        .search_cached(&path, &query, None)
        .unwrap()
        .is_some());
    std::fs::write(format!("{}.stale", path.display()), b"watch queue overflow").unwrap();
    assert!(browser.search_cached(&path, &query, None).is_err());
    assert!(browser
        .search_prepared(&path, &base, &query, None, None, &|| false)
        .is_err());
    drop((browser, base));
    cleanup(&path);
}

#[test]
fn a_partially_evicted_revision_cannot_be_replayed() {
    let db = rusqlite::Connection::open_in_memory().unwrap();
    db.execute_batch("CREATE TABLE search_revision(id INTEGER PRIMARY KEY,revision INTEGER); INSERT INTO search_revision VALUES(1,0); CREATE TABLE search_floor(id INTEGER PRIMARY KEY,revision INTEGER); INSERT INTO search_floor VALUES(1,0); CREATE TABLE search_edits(sequence INTEGER PRIMARY KEY,revision INTEGER,old_record BLOB,new_record BLOB,invalidated INTEGER);").unwrap();
    for _ in 0..8193 {
        journal::append(&db, None, None, false).unwrap();
    }
    journal::trim(&db).unwrap();
    assert!(!journal::can_replay(&db, 0).unwrap());
    assert!(journal::can_replay(&db, 1).unwrap());
    let rows: i64 = db
        .query_row("SELECT count(*) FROM search_edits", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 8192);
}
