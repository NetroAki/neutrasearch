use rusqlite::{Connection, OpenFlags};
use std::path::Path;

pub(crate) fn open(path: impl AsRef<Path>, flags: OpenFlags) -> rusqlite::Result<Connection> {
    #[cfg(unix)]
    let path = canonical_parent(path.as_ref()).map_err(|error| {
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CANTOPEN),
            Some(error.to_string()),
        )
    })?;
    Connection::open_with_flags(path, flags)
}

#[cfg(unix)]
fn canonical_parent(path: &Path) -> std::io::Result<std::path::PathBuf> {
    let name = path.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "database path has no file name",
        )
    })?;
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    // Resolve directory aliases such as macOS /var, retaining NOFOLLOW on the database itself.
    Ok(std::fs::canonicalize(parent)?.join(name))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn opens_parent_aliases_but_rejects_database_symlinks() {
        let root = std::env::temp_dir().join(format!("neutra-sqlite-path-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let actual = root.join("actual");
        std::fs::create_dir(&actual).unwrap();
        symlink(&actual, root.join("alias")).unwrap();
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NOFOLLOW;
        let path = root.join("alias/index.sqlite");
        let db = open(&path, flags).unwrap();
        db.execute_batch("CREATE TABLE sentinel(value); INSERT INTO sentinel VALUES(42);")
            .unwrap();
        drop(db);
        symlink(actual.join("index.sqlite"), actual.join("linked.sqlite")).unwrap();
        assert!(open(root.join("alias/linked.sqlite"), flags).is_err());
        let db = open(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .unwrap();
        assert_eq!(
            db.query_row("SELECT value FROM sentinel", [], |row| row.get::<_, i32>(0))
                .unwrap(),
            42
        );
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }
}
