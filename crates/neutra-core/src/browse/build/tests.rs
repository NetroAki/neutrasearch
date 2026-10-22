use super::*;
use rusqlite::Connection;

#[test]
fn replacement_preserves_wal_readers_snapshots_and_live_writes() {
    let root = std::env::temp_dir().join(format!("ns-browse-publish-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let destination = root.join("catalog.sqlite");
    let staged = root.join("catalog.new");
    let old = Connection::open(&destination).unwrap();
    old.busy_timeout(LOCK_TIMEOUT).unwrap();
    old.execute_batch(
        "PRAGMA journal_mode=WAL;
        CREATE TABLE metadata(generation INTEGER NOT NULL);
        CREATE TABLE short_counts(code INTEGER PRIMARY KEY, matched INTEGER NOT NULL);
        INSERT INTO metadata VALUES(1);",
    )
    .unwrap();
    {
        let tx = old.unchecked_transaction().unwrap();
        for code in 0..1332 {
            tx.execute("INSERT INTO short_counts VALUES(?1,?2)", [code, code])
                .unwrap();
        }
        tx.commit().unwrap();
    }
    let reader = Connection::open(&destination).unwrap();
    reader.busy_timeout(LOCK_TIMEOUT).unwrap();
    let snapshot = reader.unchecked_transaction().unwrap();
    assert_eq!(
        snapshot
            .query_row("SELECT count(*) FROM short_counts", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1332
    );
    // Establish the read snapshot before publishing, then commit another WAL change.
    old.execute("UPDATE metadata SET generation=2", []).unwrap();

    let fresh = Connection::open(&staged).unwrap();
    fresh
        .execute_batch(
            "CREATE TABLE metadata(generation INTEGER NOT NULL);
        CREATE TABLE short_counts(code INTEGER PRIMARY KEY, matched INTEGER NOT NULL);
        INSERT INTO metadata VALUES(3);",
        )
        .unwrap();
    {
        let tx = fresh.unchecked_transaction().unwrap();
        for code in 0..1332 {
            tx.execute("INSERT INTO short_counts VALUES(?1,?2)", [code, code + 10])
                .unwrap();
        }
        tx.commit().unwrap();
    }
    drop(fresh);

    replace(&staged, &destination).unwrap();
    assert_eq!(
        snapshot
            .query_row("SELECT generation FROM metadata", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        snapshot
            .query_row("SELECT count(*) FROM short_counts", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1332
    );
    snapshot.commit().unwrap();
    assert_eq!(
        old.query_row("SELECT generation FROM metadata", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        3
    );
    old.execute("INSERT INTO short_counts VALUES(2000,42)", [])
        .unwrap();
    assert_eq!(
        old.query_row("SELECT count(*) FROM short_counts", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1333
    );
    drop(old);
    drop(reader);

    let reopened = Connection::open(&destination).unwrap();
    assert_eq!(
        reopened
            .query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    assert_eq!(
        reopened
            .query_row("SELECT count(*) FROM short_counts", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1333
    );
    assert_eq!(
        reopened
            .query_row(
                "SELECT count(*) FROM short_counts WHERE code BETWEEN 0 AND 1331",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1332
    );
    drop(reopened);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn replacement_creates_a_missing_destination() {
    let root = std::env::temp_dir().join(format!("ns-browse-publish-new-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let destination = root.join("catalog.sqlite");
    let staged = root.join("catalog.new");
    let fresh = Connection::open(&staged).unwrap();
    fresh
        .execute_batch("CREATE TABLE metadata(generation INTEGER); INSERT INTO metadata VALUES(9);")
        .unwrap();
    drop(fresh);
    replace(&staged, &destination).unwrap();
    let reopened = Connection::open(&destination).unwrap();
    assert_eq!(
        reopened
            .query_row("SELECT generation FROM metadata", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        9
    );
    assert!(!staged.exists());
    drop(reopened);
    let _ = std::fs::remove_dir_all(root);
}
