use super::*;
use crate::scan::{
    MAX_DELTA_CHANGES, MAX_INDEX_PATH_BYTES, MAX_QUERY_RESULTS, MAX_QUERY_TERMS,
    MAX_QUERY_TEXT_BYTES, MAX_SCAN_MOUNTS,
};
use anyhow::Result;
use neutra_core::{DeltaChange, Query};

pub(crate) fn validate_query(query: &Query) -> Result<()> {
    if query.limit == 0 || query.limit > MAX_QUERY_RESULTS {
        anyhow::bail!("query limit must be between 1 and {MAX_QUERY_RESULTS}");
    }
    if query.terms.len() > MAX_QUERY_TERMS {
        anyhow::bail!("query exceeds the {MAX_QUERY_TERMS}-term limit");
    }
    if query.scope_roots.len() > MAX_SCAN_MOUNTS
        || query
            .scope_roots
            .iter()
            .any(|root| !std::path::Path::new(root).is_absolute())
    {
        anyhow::bail!("query scopes must be absolute and limited to {MAX_SCAN_MOUNTS} roots");
    }
    let text_bytes = query.terms.iter().map(String::len).sum::<usize>()
        + query.exts.iter().map(String::len).sum::<usize>()
        + query.scope_roots.iter().map(String::len).sum::<usize>()
        + query.under.as_ref().map_or(0, String::len);
    if text_bytes > MAX_QUERY_TEXT_BYTES {
        anyhow::bail!("query text exceeds the {MAX_QUERY_TEXT_BYTES}-byte limit");
    }
    Ok(())
}

pub(crate) fn validate_delta_changes(changes: &[DeltaChange]) -> Result<()> {
    if changes.len() > MAX_DELTA_CHANGES {
        anyhow::bail!("delta batch exceeds the {MAX_DELTA_CHANGES}-change limit");
    }
    for change in changes {
        let path = match change {
            DeltaChange::Upsert(record) => record.path.as_ref(),
            DeltaChange::Remove(path) | DeltaChange::RemovePrefix(path) => path.as_ref(),
        };
        if path.is_empty() || path.len() > MAX_INDEX_PATH_BYTES {
            anyhow::bail!("delta path length is outside the supported range");
        }
        if !safe_absolute_path(path) {
            anyhow::bail!("delta paths must be absolute and normalized");
        }
    }
    Ok(())
}
