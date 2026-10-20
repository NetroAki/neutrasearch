use super::ordered::Stream;
use super::BrowserIndex;
use crate::{CompactIndex, DeltaIndex, FileRecord, Query, SearchHit, SearchStats, SortKey};
use rusqlite::types::Value;
use std::io;
use std::time::Instant;
mod short;

pub(super) fn search(
    browser: &BrowserIndex,
    base: &CompactIndex,
    query: &Query,
    delta: Option<&DeltaIndex>,
    offset: usize,
    after: Option<&FileRecord>,
) -> io::Result<(Vec<SearchHit>, SearchStats)> {
    if !query.terms.is_empty() || query.regex.is_some() || query.limit == 0 {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "browse orders require a bounded term-free query",
        ));
    }
    let started = Instant::now();
    let take = query
        .limit
        .checked_add(offset)
        .ok_or_else(|| io::Error::other("page offset overflow"))?;
    let matcher = query.matcher()?;
    let mut stream = Stream::new(browser, query, after, delta, take)?;
    let mut hits = Vec::with_capacity(take.min(10_000));
    while hits.len() < take {
        let Some(record) = stream.next()? else {
            break;
        };
        if let Some(score) = matcher.score(&record) {
            hits.push(SearchHit { record, score });
        }
    }
    let matched = hits.len() as u64;
    hits.drain(..offset.min(hits.len()));
    // Filtered totals are a lower bound; fetching a page never counts millions of rows.
    let stats = SearchStats {
        scanned: base.len(),
        matched: if unfiltered(query) {
            base.len()
        } else {
            matched
        },
        wall_us: started.elapsed().as_micros() as u64,
    };
    Ok((hits, stats))
}

fn unfiltered(query: &Query) -> bool {
    query.kinds.is_empty()
        && query.exts.is_empty()
        && query.fss.is_empty()
        && query.min_size.is_none()
        && query.max_size.is_none()
        && query.under.is_none()
        && (query.scope_roots.is_empty() || query.scope_roots == ["/"])
        && !query.executable_only
        && query.exclude_roots.is_empty()
}

pub(in crate::browse) fn ordering(sort: SortKey) -> (&'static str, &'static str) {
    match sort {
        SortKey::NameAsc => ("name COLLATE NOCASE ASC,path ASC", "by_name"),
        SortKey::NameDesc => ("name COLLATE NOCASE DESC,path DESC", "by_name"),
        SortKey::PathAsc => ("path ASC", "by_path"),
        SortKey::PathDesc => ("path DESC", "by_path"),
        SortKey::SizeAsc => ("bytes ASC", "by_size"),
        SortKey::SizeDesc => ("bytes DESC", "by_size"),
        SortKey::MtimeAsc => ("mtime ASC", "by_modified"),
        SortKey::MtimeDesc | SortKey::Relevance => ("mtime DESC", "by_modified"),
    }
}

pub(in crate::browse) fn tie_path_direction(sort: SortKey) -> &'static str {
    match sort {
        SortKey::NameDesc
        | SortKey::PathDesc
        | SortKey::SizeDesc
        | SortKey::MtimeDesc
        | SortKey::Relevance => "DESC",
        _ => "ASC",
    }
}

pub(in crate::browse) fn cursor_clause(
    sort: SortKey,
    record: &FileRecord,
    values: &mut Vec<Value>,
) -> String {
    let (key, value, operator, path_operator) = match sort {
        SortKey::NameAsc => (
            "name COLLATE NOCASE",
            Value::Text(record.name().to_owned()),
            ">",
            ">",
        ),
        SortKey::NameDesc => (
            "name COLLATE NOCASE",
            Value::Text(record.name().to_owned()),
            "<",
            "<",
        ),
        SortKey::SizeAsc => (
            "bytes",
            Value::Blob(record.disk_bytes().to_be_bytes().to_vec()),
            ">",
            ">",
        ),
        SortKey::SizeDesc => (
            "bytes",
            Value::Blob(record.disk_bytes().to_be_bytes().to_vec()),
            "<",
            "<",
        ),
        SortKey::MtimeAsc => (
            "mtime",
            Value::Integer(if record.mtime > 4_102_444_800 {
                0
            } else {
                record.mtime
            }),
            ">",
            ">",
        ),
        SortKey::MtimeDesc | SortKey::Relevance => (
            "mtime",
            Value::Integer(if record.mtime > 4_102_444_800 {
                0
            } else {
                record.mtime
            }),
            "<",
            "<",
        ),
        SortKey::PathAsc | SortKey::PathDesc => {
            values.push(Value::Text(record.path.to_string()));
            return format!(
                "path {} ?",
                if sort == SortKey::PathAsc { ">" } else { "<" }
            );
        }
    };
    values.extend([value.clone(), value, Value::Text(record.path.to_string())]);
    format!("({key}{operator}? OR ({key}=? AND path{path_operator}?))")
}

mod conditions;
pub(in crate::browse) use conditions::filters;
