use super::columns::Columns;
use super::{sql_error, BrowserIndex, SCHEMA_VERSION};
use crate::CompactIndex;
use fs2::FileExt;
use rusqlite::params;
use std::io;
use std::path::Path;

const SCHEMA: &str = "
PRAGMA journal_mode=OFF;
PRAGMA synchronous=OFF;
PRAGMA cache_size=-16384;
PRAGMA temp_store=FILE;
PRAGMA mmap_size=0;
CREATE TABLE metadata(generation TEXT NOT NULL,version INTEGER NOT NULL,base_generation TEXT NOT NULL);
CREATE TABLE entries(id INTEGER PRIMARY KEY,path TEXT NOT NULL,name TEXT NOT NULL,
 bytes BLOB NOT NULL,mtime INTEGER NOT NULL,kind INTEGER NOT NULL,ext TEXT NOT NULL,
 fs TEXT NOT NULL,executable INTEGER NOT NULL,record BLOB);
CREATE INDEX by_path ON entries(path);
CREATE INDEX by_name ON entries(name COLLATE NOCASE,path);
CREATE INDEX by_quick ON entries(name COLLATE NOCASE,mtime DESC,path DESC);
CREATE INDEX by_size ON entries(bytes,path);
CREATE INDEX by_modified ON entries(mtime,path);
CREATE INDEX by_ext ON entries(ext,id);
CREATE INDEX by_programs ON entries(executable,kind,id);
CREATE INDEX by_initial ON entries(substr(name,1,1) COLLATE NOCASE,mtime DESC,path DESC,name);
CREATE TABLE tomb(id INTEGER PRIMARY KEY);
CREATE TABLE tomb_range(low INTEGER PRIMARY KEY,high INTEGER NOT NULL);
CREATE TABLE tomb_revision(id INTEGER PRIMARY KEY CHECK(id=1),revision INTEGER NOT NULL);
INSERT INTO tomb_revision VALUES(1,0);
";

pub(super) fn ensure(index_path: &Path, generation: u64) -> io::Result<()> {
    if BrowserIndex::is_current(index_path, generation) {
        return Ok(());
    }
    let destination = BrowserIndex::path_for(index_path);
    let lock_path = destination.with_extension("browse.lock");
    let lock = open_browse_lock(&lock_path)?
        .into_inner()
        .map_err(|e| e.into_error())?;
    lock.try_lock_exclusive()?;
    if BrowserIndex::is_current(index_path, generation) {
        return Ok(());
    }
    let base = CompactIndex::open_fast(index_path)?;
    base.require_current_order()?;
    if base.generation() != generation {
        return Err(io::Error::other(
            "index replaced before building browse orders",
        ));
    }
    let temporary = destination.with_extension("browse.new");
    let columns = Columns::path_for(&destination);
    let columns_temporary = crate::compact::suffix_path(&columns, ".new");
    for stale in [&temporary, &columns_temporary] {
        if stale.exists() {
            std::fs::remove_file(stale)?;
        }
    }
    let result = build(&base, &temporary, &columns_temporary, generation);
    if let Err(error) = result {
        let _ = std::fs::remove_file(&temporary);
        let _ = std::fs::remove_file(&columns_temporary);
        return Err(error);
    }
    if CompactIndex::generation_on_disk(index_path)? != generation {
        let _ = std::fs::remove_file(&temporary);
        let _ = std::fs::remove_file(&columns_temporary);
        return Err(io::Error::other(
            "index replaced while building browse orders",
        ));
    }
    super::updates::pin_base(index_path)?;
    crate::compact_build::replace_file(&columns_temporary, &columns)?;
    crate::compact_build::replace_file(&temporary, &destination)?;
    crate::compact_build::sync_parent(&destination)?;
    let wal = index_path.with_extension("delta");
    if std::fs::metadata(&wal).is_ok_and(|m| m.len() == crate::DELTA_HEADER_BYTES) {
        let empty = crate::DeltaIndex::open_snapshot(&wal, generation)?;
        if empty.wal_bytes() == crate::DELTA_HEADER_BYTES {
            BrowserIndex::mark_delta(index_path, &empty)?;
        }
    }
    Ok(())
}

/// Open the browse prep lock, reusing a stale unlocked file from a failed
/// prep. open_private uses create_new, so a leftover lock file would
/// otherwise make every retry fail with AlreadyExists.
fn open_browse_lock(path: &Path) -> io::Result<std::io::BufWriter<std::fs::File>> {
    match crate::compact_build::open_private(path) {
        Ok(file) => Ok(file),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let mut options = std::fs::OpenOptions::new();
            options.read(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
            }
            let file = options.open(path)?;
            if !file.metadata()?.is_file() {
                return Err(io::Error::other("browse lock is not a regular file"));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                let metadata = file.metadata()?;
                if metadata.mode() & 0o077 != 0 || metadata.nlink() != 1 {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "browse lock must be private and single-linked",
                    ));
                }
            }
            Ok(std::io::BufWriter::with_capacity(8 * 1024, file))
        }
        Err(error) => Err(error),
    }
}

fn build(base: &CompactIndex, path: &Path, columns: &Path, generation: u64) -> io::Result<()> {
    let built = super::columns::build(base, columns)?;
    drop(crate::compact_build::open_private(path)?);
    let mut db = crate::sqlite::open(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(sql_error)?;
    db.execute_batch(SCHEMA).map_err(sql_error)?;
    let transaction = db.transaction().map_err(sql_error)?;
    built.counts.publish(&transaction, built.unix_paths)?;
    transaction
        .execute(
            "INSERT INTO metadata VALUES(?,?,?)",
            params![
                generation.to_string(),
                SCHEMA_VERSION,
                generation.to_string()
            ],
        )
        .map_err(sql_error)?;
    transaction.commit().map_err(sql_error)?;
    db.close().map_err(|(_, e)| sql_error(e))?;
    // Windows FlushFileBuffers requires write access.
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)?
        .sync_all()
}
