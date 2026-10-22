//! Complete disk-backed sort orders. Compressed records stay in the base;
//! this sidecar contains sortable keys and record addresses, not a resident copy.

mod build;
mod catalog;
mod columns;
mod cursor;
mod hierarchy;
mod ordered;
mod query;
mod quick;
mod search_cache;
mod short_counts;
#[cfg(test)]
mod tests;
mod updates;

use crate::{CompactIndex, DeltaIndex, FileRecord, Query, SearchHit, SearchStats};
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};
use std::{io, sync::Mutex};

// Reversing an order also reverses its path tie-break, so one B-tree covers
// both directions without separate descending indexes.
const SCHEMA_VERSION: i64 = 4;

pub struct BrowserIndex {
    db: Connection,
    generation: u64,
    base: CompactIndex,
    cols: columns::Columns,
    tombstone_cache: std::sync::Mutex<Option<(i64, std::sync::Arc<catalog::Tombstones>)>>,
    decoded_blocks: Mutex<columns::DecodedBlocks>,
}

pub(crate) fn sql_error(error: rusqlite::Error) -> io::Error {
    io::Error::other(error)
}

impl BrowserIndex {
    /// Recover a new WAL from a synchronized catalog whose original base
    /// is still published. Stop its writer before invoking or installing it.
    pub fn recover_delta(index: &Path, output: &Path) -> io::Result<()> {
        cursor::recovery::rebuild(index, output)
    }

    pub fn path_for(index: &Path) -> PathBuf {
        let mut path = index.as_os_str().to_os_string();
        path.push(".browse");
        path.into()
    }

    pub fn open(index: &Path, generation: u64) -> io::Result<Self> {
        let db = crate::sqlite::open(
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
        base.require_current_order()?;
        if base.generation().to_string() != found.2 {
            return Err(io::Error::other("pinned browse base generation mismatch"));
        }
        let cols = columns::Columns::open(
            &columns::Columns::path_for(&Self::path_for(index)),
            base.generation(),
            base.len(),
        )?;
        Ok(Self {
            db,
            generation,
            base,
            cols,
            tombstone_cache: std::sync::Mutex::new(None),
            decoded_blocks: Mutex::new(columns::DecodedBlocks::default()),
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
