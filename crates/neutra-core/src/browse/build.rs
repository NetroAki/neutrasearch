use super::{sql_error, BrowserIndex, SCHEMA_VERSION};
use crate::{CompactIndex, FileRecord};
use fs2::FileExt;
use rusqlite::{params, Connection};
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
";

pub(super) fn ensure(index_path: &Path, generation: u64) -> io::Result<()> {
    if super::updates::upgrade(index_path, generation)? {
        return Ok(());
    }
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
    if base.generation() != generation {
        return Err(io::Error::other(
            "index replaced before building browse orders",
        ));
    }
    let temporary = destination.with_extension("browse.new");
    if temporary.exists() {
        std::fs::remove_file(&temporary)?;
    }
    drop(crate::compact_build::open_private(&temporary)?);
    let result = build(&base, &temporary, generation);
    if let Err(error) = result {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    if CompactIndex::generation_on_disk(index_path)? != generation {
        let _ = std::fs::remove_file(&temporary);
        return Err(io::Error::other(
            "index replaced while building browse orders",
        ));
    }
    super::updates::pin_base(index_path)?;
    crate::compact_build::replace_file(&temporary, &destination)?;
    crate::compact_build::sync_parent(&destination)
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

fn build(base: &CompactIndex, path: &Path, generation: u64) -> io::Result<()> {
    let mut db = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(sql_error)?;
    db.execute_batch(SCHEMA).map_err(sql_error)?;
    let transaction = db.transaction().map_err(sql_error)?;
    {
        let mut insert = transaction
            .prepare_cached("INSERT INTO entries VALUES(?,?,?,?,?,?,?,?,?,NULL)")
            .map_err(sql_error)?;
        for block in 0..base.block_count() {
            for (slot, record) in base.read_block(block as u32)?.iter().enumerate() {
                let id = (block * crate::compact::BLOCK_RECORDS + slot + 1) as i64;
                insert_record(&mut insert, id, record)?;
            }
            base.release_block(block as u32);
            if block % 100_000 == 0 {
                eprintln!(
                    "Preparing browse orders: {} / {} records",
                    block * crate::compact::BLOCK_RECORDS,
                    base.len()
                );
            }
        }
    }
    transaction.commit().map_err(sql_error)?;
    for (name, columns) in [
        ("by_path", "path"),
        ("by_name", "name COLLATE NOCASE,path"),
        ("by_size", "bytes,path"),
        ("by_modified", "mtime,path"),
        ("by_ext", "ext,id"),
        ("by_programs", "executable,kind,id"),
    ] {
        eprintln!("Preparing browse order: {name}");
        db.execute_batch(&format!("CREATE INDEX {name} ON entries({columns})"))
            .map_err(sql_error)?;
    }
    db.execute(
        "INSERT INTO metadata VALUES(?,?,?)",
        params![
            generation.to_string(),
            SCHEMA_VERSION,
            generation.to_string()
        ],
    )
    .map_err(sql_error)?;
    db.close().map_err(|(_, e)| sql_error(e))?;
    std::fs::File::open(path)?.sync_all()
}

fn insert_record(
    insert: &mut rusqlite::Statement<'_>,
    id: i64,
    record: &FileRecord,
) -> io::Result<()> {
    let mtime = if record.mtime > 4_102_444_800 {
        0
    } else {
        record.mtime
    };
    insert
        .execute(params![
            id,
            &*record.path,
            record.name(),
            &record.disk_bytes().to_be_bytes()[..],
            mtime,
            record.kind as i64,
            record.extension().to_ascii_lowercase(),
            record.fs.label(),
            i64::from(crate::query::is_executable(record))
        ])
        .map_err(sql_error)?;
    Ok(())
}
