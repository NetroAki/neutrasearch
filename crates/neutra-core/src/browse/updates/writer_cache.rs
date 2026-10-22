use super::base_path;
use crate::browse::columns::DecodedBlocks;
use crate::CompactIndex;
use std::{
    cell::RefCell,
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[cfg(test)]
mod tests;

#[derive(Clone, PartialEq, Eq)]
struct Stamp {
    bytes: u64,
    modified: std::time::SystemTime,
    #[cfg(unix)]
    identity: (u64, u64),
}

fn stamp(path: &Path) -> io::Result<Stamp> {
    let meta = std::fs::metadata(path)?;
    Ok(Stamp {
        bytes: meta.len(),
        modified: meta.modified()?,
        #[cfg(unix)]
        identity: {
            use std::os::unix::fs::MetadataExt;
            (meta.dev(), meta.ino())
        },
    })
}

#[derive(Clone)]
pub(super) struct Held {
    path: PathBuf,
    generation: String,
    stamp: Stamp,
    pub(super) base: Arc<CompactIndex>,
    pub(super) decoded: Arc<Mutex<DecodedBlocks>>,
}

thread_local! {
    static HELD: RefCell<Option<Held>> = const { RefCell::new(None) };
}

#[cfg(test)]
pub(in crate::browse) fn clear_for_test() {
    HELD.with(|cache| *cache.borrow_mut() = None);
}

pub(super) fn open(index: &Path, generation: &str) -> io::Result<Held> {
    let path = std::fs::canonicalize(base_path(index))?;
    let before = stamp(&path)?;
    if let Some(found) = HELD.with(|cache| {
        cache
            .borrow()
            .as_ref()
            .filter(|held| {
                held.path == path && held.generation == generation && held.stamp == before
            })
            .cloned()
    }) {
        return Ok(found);
    }
    let base = Arc::new(CompactIndex::open_fast(&path)?);
    if base.generation().to_string() != generation {
        return Err(io::Error::other("pinned browse base generation mismatch"));
    }
    if before != stamp(&path)? {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "pinned browse base changed during open",
        ));
    }
    let held = Held {
        path,
        generation: generation.to_owned(),
        stamp: before,
        base,
        decoded: Arc::new(Mutex::new(DecodedBlocks::new())),
    };
    HELD.with(|cache| *cache.borrow_mut() = Some(held.clone()));
    Ok(held)
}
