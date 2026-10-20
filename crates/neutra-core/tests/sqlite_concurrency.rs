//! The deadlock was in the Unix lock path, so the stress test is Unix-only.
#![cfg(unix)]

use rusqlite::{Connection, OpenFlags};
use std::{
    path::Path,
    process::Command,
    sync::Arc,
    time::{Duration, Instant},
};

#[test]
fn wal_readers_and_connection_churn_do_not_deadlock() {
    if std::env::var_os("NEUTRA_SQLITE_STRESS_CHILD").is_some() {
        stress();
        return;
    }
    // SQLite 3.51.1's unixLock/findReusableFd lock inversion stalled all
    // filesystem updates while systemd still reported a running service.
    assert!(
        rusqlite::version_number() >= 3_051_002,
        "SQLite deadlock fix required"
    );
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "wal_readers_and_connection_churn_do_not_deadlock",
            "--nocapture",
        ])
        .env("NEUTRA_SQLITE_STRESS_CHILD", "1")
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("SQLite connection churn deadlocked");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn connect(path: &Path) -> Connection {
    let db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap();
    db.busy_timeout(Duration::from_secs(5)).unwrap();
    db
}

fn stress() {
    let root = std::env::temp_dir().join(format!("ns-sqlite-churn-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    // NOFOLLOW rejects directory aliases such as macOS /var.
    let path = root.canonicalize().unwrap().join("live.sqlite");
    let db = connect(&path);
    db.execute_batch(
        "PRAGMA journal_mode=WAL; CREATE TABLE live(value INTEGER); INSERT INTO live VALUES(0);",
    )
    .unwrap();
    drop(db);
    let start = Arc::new(std::sync::Barrier::new(4));
    std::thread::scope(|scope| {
        for reader in 0..4 {
            let path = &path;
            let start = Arc::clone(&start);
            scope.spawn(move || churn(path, reader, &start));
        }
    });
    let db = connect(&path);
    assert_eq!(
        db.query_row("SELECT value FROM live", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        499
    );
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}

fn churn(path: &Path, reader: usize, start: &std::sync::Barrier) {
    start.wait();
    for turn in 0..500 {
        let db = connect(path);
        if reader == 0 {
            db.execute("UPDATE live SET value=?", [turn]).unwrap();
        } else {
            db.execute_batch("BEGIN; SELECT * FROM live; COMMIT;")
                .unwrap();
        }
    }
}
