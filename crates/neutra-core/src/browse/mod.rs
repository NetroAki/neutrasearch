//! Complete disk-backed sort orders. Compressed records stay in the base;
//! this sidecar contains sortable keys and record addresses, not a resident copy.

mod build;
mod cursor;
mod hierarchy;
mod query;
mod recovery;
#[cfg(test)]
mod tests;
mod updates;

use crate::{CompactIndex, DeltaIndex, FileRecord, Query, SearchHit, SearchStats};
use rusqlite::{Connection, OpenFlags};
use std::io;
use std::path::{Path, PathBuf};

// Reversing an order also reverses its path tie-break, so one B-tree covers
// both directions without separate descending indexes.
const SCHEMA_VERSION: i64 = 3;

pub struct BrowserIndex {
    db: Connection,
    generation: u64,
    base: CompactIndex,
}

pub(crate) fn sql_error(error: rusqlite::Error) -> io::Error {
    io::Error::other(error)
}

impl BrowserIndex {
    /// Recover a new WAL from a synchronized catalog whose original base
    /// is still published. Stop its writer before invoking or installing it.
    pub fn recover_delta(index: &Path, output: &Path) -> io::Result<()> {
        recovery::rebuild(index, output)
    }

    /// Resolve a native watcher lookup through the path B-tree and decode
    /// one original block, rather than binary-searching compressed blocks.
    pub fn record_by_path(&self, path: &str) -> io::Result<Option<FileRecord>> {
        self.record_by_path_cached(path, &mut None)
    }

    pub(super) fn record_by_path_cached(
        &self,
        path: &str,
        cache: &mut Option<(u32, Vec<FileRecord>)>,
    ) -> io::Result<Option<FileRecord>> {
        use rusqlite::OptionalExtension;
        let found: Option<(i64, Option<Vec<u8>>)> = self
            .db
            .prepare_cached("SELECT id,record FROM entries WHERE path=? LIMIT 1")
            .map_err(sql_error)?
            .query_row([path], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()
            .map_err(sql_error)?;
        let Some((id, bytes)) = found else {
            return Ok(None);
        };
        if let Some(bytes) = bytes {
            return bincode::deserialize(&bytes)
                .map(Some)
                .map_err(io::Error::other);
        }
        if id <= 0 || id as u64 > self.base.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid browse record address",
            ));
        }
        let address = (id - 1) as usize;
        let block = (address / crate::compact::BLOCK_RECORDS) as u32;
        if cache.as_ref().map(|(held, _)| *held) != Some(block) {
            let records = self.base.read_block(block)?;
            self.base.release_block(block);
            *cache = Some((block, records));
        }
        cache
            .as_ref()
            .expect("requested block was cached")
            .1
            .get(address % crate::compact::BLOCK_RECORDS)
            .cloned()
            .map(Some)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid browse record slot"))
    }

    pub fn path_for(index: &Path) -> PathBuf {
        let mut path = index.as_os_str().to_os_string();
        path.push(".browse");
        path.into()
    }

    pub fn open(index: &Path, generation: u64) -> io::Result<Self> {
        let db = Connection::open_with_flags(
            Self::path_for(index),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(sql_error)?;
        db.execute_batch("PRAGMA cache_size=-4096; PRAGMA mmap_size=0; PRAGMA temp_store=FILE;")
            .map_err(sql_error)?;
        let found: (String, i64, String) = db
            .query_row(
                "SELECT generation,version,base_generation FROM metadata",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(sql_error)?;
        if found.0 != generation.to_string() || found.1 != SCHEMA_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "browse index is stale",
            ));
        }
        let base = CompactIndex::open_fast(&updates::base_path(index))?;
        if base.generation().to_string() != found.2 {
            return Err(io::Error::other("pinned browse base generation mismatch"));
        }
        Ok(Self {
            db,
            generation,
            base,
        })
    }

    pub fn is_current(index: &Path, generation: u64) -> bool {
        Self::open(index, generation).is_ok()
    }

    pub fn is_building(index: &Path) -> bool {
        use fs2::FileExt;
        let path = Self::path_for(index).with_extension("browse.lock");
        let Ok(file) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
        else {
            return false;
        };
        match FileExt::try_lock_shared(&file) {
            Ok(()) => {
                let _ = FileExt::unlock(&file);
                false
            }
            Err(error) => error.kind() == io::ErrorKind::WouldBlock,
        }
    }

    pub fn ensure(index: &Path, generation: u64) -> io::Result<()> {
        build::ensure(index, generation)
    }

    pub fn search(
        &self,
        base: &CompactIndex,
        query: &Query,
        delta: Option<&DeltaIndex>,
    ) -> io::Result<(Vec<SearchHit>, SearchStats)> {
        self.search_page(base, query, delta, 0)
    }

    pub fn search_page(
        &self,
        base: &CompactIndex,
        query: &Query,
        delta: Option<&DeltaIndex>,
        offset: usize,
    ) -> io::Result<(Vec<SearchHit>, SearchStats)> {
        if base.generation() != self.generation {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "browse/base generation mismatch",
            ));
        }
        query::search(self, base, query, delta, offset, None)
    }

    pub fn search_after(
        &self,
        base: &CompactIndex,
        query: &Query,
        delta: Option<&DeltaIndex>,
        after: Option<&FileRecord>,
    ) -> io::Result<(Vec<SearchHit>, SearchStats)> {
        if base.generation() != self.generation {
            return Err(io::Error::other("browse/base generation mismatch"));
        }
        query::search(self, base, query, delta, 0, after)
    }
}
