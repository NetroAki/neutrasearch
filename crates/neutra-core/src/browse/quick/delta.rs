use crate::{DeltaIndex, Query, SearchHit};
use std::io;

pub(super) fn shadowed(delta: Option<&DeltaIndex>, path: &str) -> io::Result<bool> {
    delta.map_or(Ok(false), |delta| delta.shadows(path))
}

pub(super) fn merge(
    delta: &DeltaIndex,
    query: &Query,
    hits: &mut Vec<SearchHit>,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> io::Result<()> {
    let matcher = query.matcher()?;
    let compare = |a: &SearchHit, b: &SearchHit| {
        crate::matcher::compare_records(query.sort, &(a.score, &a.record), &(b.score, &b.record))
    };
    delta.for_each_upsert(|record| {
        if cancelled() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "search superseded",
            ));
        }
        if let Some(score) = matcher.score(&record) {
            hits.push(SearchHit { score, record });
        }
        if hits.len() >= 64 {
            hits.sort_unstable_by(compare);
            hits.truncate(query.limit);
        }
        Ok(())
    })?;
    hits.sort_unstable_by(compare);
    hits.truncate(query.limit);
    Ok(())
}
