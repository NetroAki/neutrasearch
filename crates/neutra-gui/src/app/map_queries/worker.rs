use neutra_core::{BrowserIndex, CompactIndex, DeltaIndex, FileRecord, Query};
use std::{io, path::Path};

pub(super) fn search(
    path: &Path,
    index: &CompactIndex,
    query: &Query,
    cancelled: &(impl Fn() -> bool + Sync),
) -> io::Result<Vec<FileRecord>> {
    if cancelled() {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "map search superseded",
        ));
    }
    let typed = !query.terms.is_empty() || query.regex.is_some();
    let browser = if typed {
        None
    } else {
        Some(BrowserIndex::open(path, index.generation())?)
    };
    let covered = browser
        .as_ref()
        .map(|browser| browser.covers_delta(path))
        .transpose()?
        .unwrap_or(false);
    let delta = if covered {
        None
    } else {
        match DeltaIndex::open_snapshot(&path.with_extension("delta"), index.generation()) {
            Ok(delta) => Some(delta),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        }
    };
    let (hits, _) = if let Some(browser) = browser {
        browser.search(index, query, delta.as_ref())?
    } else {
        index.search_interruptible(query, delta.as_ref(), cancelled)?
    };
    Ok(hits.into_iter().map(|hit| hit.record).collect())
}
