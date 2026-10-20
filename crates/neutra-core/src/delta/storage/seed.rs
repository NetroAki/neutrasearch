//! Seeds a private snapshot from the writer's resolved cache so a reader
//! replays only the WAL tail instead of the whole log.

use super::*;
#[cfg(test)]
use std::fs::File;

/// SQLite copies a coherent view without opening raw descriptors on the live
/// database, which could release POSIX locks held by other connections.
/// Failure leaves an empty snapshot for authoritative WAL replay.
pub(super) fn seed_snapshot(shared: &Path, snapshot: &Path) {
    let _ = try_seed(shared, snapshot);
}

fn try_seed(shared: &Path, snapshot: &Path) -> io::Result<()> {
    if !shared.is_file() {
        return Ok(());
    }
    let db = crate::sqlite::open(
        shared,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(sql_error)?;
    db.busy_timeout(std::time::Duration::from_secs(30))
        .map_err(sql_error)?;
    db.execute_batch("PRAGMA synchronous=OFF;")
        .map_err(sql_error)?;
    // NOFOLLOW also rejects directory aliases such as macOS /var in the target.
    let snapshot = match (snapshot.parent(), snapshot.file_name()) {
        (Some(parent), Some(name)) => parent
            .canonicalize()
            .map(|parent| parent.join(name))
            .unwrap_or_else(|_| snapshot.to_path_buf()),
        _ => snapshot.to_path_buf(),
    };
    db.execute("VACUUM main INTO ?1", [snapshot.to_string_lossy().as_ref()])
        .map_err(sql_error)?;
    Ok(())
}

/// Remove snapshot files left behind by processes that no longer exist.
pub(super) fn remove_dead_snapshots(overlay_dir: &Path, base_name: &std::ffi::OsStr) {
    let prefix = format!("{}.snapshot-", base_name.to_string_lossy());
    let Ok(entries) = std::fs::read_dir(overlay_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(rest) = name.strip_prefix(&prefix) else {
            continue;
        };
        let Some(pid) = rest
            .split('-')
            .next()
            .and_then(|pid| pid.parse::<u32>().ok())
        else {
            continue;
        };
        if pid != std::process::id() && !process_alive(pid) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn process_alive(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        Path::new(&format!("/proc/{pid}")).exists()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_cache_snapshot_preserves_rows_without_abandoned_pages() {
        let root = std::env::temp_dir().join(format!("ns-sparse-seed-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let shared = root.join("shared.sqlite");
        let snapshot = root.join("snapshot.sqlite");
        let db = crate::sqlite::open(
            &shared,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )
        .unwrap();
        db.execute_batch("CREATE TABLE rows(id INTEGER PRIMARY KEY,data BLOB); WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<6000) INSERT INTO rows SELECT x,randomblob(1024) FROM n; DELETE FROM rows WHERE id>10;").unwrap();
        let expected: Vec<u8> = db
            .query_row("SELECT data FROM rows WHERE id=1", [], |row| row.get(0))
            .unwrap();
        File::create(&snapshot).unwrap();
        try_seed(&shared, &snapshot).unwrap();
        let copy = crate::sqlite::open(&snapshot, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        assert_eq!(
            copy.query_row("SELECT count(*) FROM rows", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            10
        );
        assert_eq!(
            copy.query_row("SELECT data FROM rows WHERE id=1", [], |row| row
                .get::<_, Vec<u8>>(0))
                .unwrap(),
            expected
        );
        assert!(
            std::fs::metadata(&snapshot).unwrap().len()
                < std::fs::metadata(&shared).unwrap().len() / 10
        );
        drop(copy);
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }
}
