use neutra_core::{BrowserIndex, CompactIndex, DeltaIndex, MatchFields, Query, SortKey};
use std::{io, path::PathBuf, time::Instant};

fn main() -> io::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = PathBuf::from(
        args.next()
            .ok_or_else(|| io::Error::other("index path required"))?,
    );
    let mut query = Query::parse(&args.next().unwrap_or_else(|| "a".into()));
    query.match_fields = MatchFields::Name;
    query.limit = 1000;
    query.sort = match args.next().as_deref() {
        Some("name") => SortKey::NameAsc,
        Some("size") => SortKey::SizeDesc,
        Some("modified") => SortKey::MtimeDesc,
        Some("path") => SortKey::PathAsc,
        Some("relevance") | None => SortKey::Relevance,
        _ => return Err(io::Error::other("unknown sort")),
    };
    let warm_only = args.next().as_deref() == Some("warm");
    let start = Instant::now();
    let base = CompactIndex::open_fast(&path)?;
    let browser = BrowserIndex::open(&path, base.generation())?;
    let cached = browser.search_cached(&path, &query, None)?;
    let fast = match cached {
        Some(found) => Some(found),
        None => browser.search_short(&path, &query, None, &|| false)?,
    };
    let result = if let Some(found) = fast {
        found
    } else if warm_only {
        return Err(io::Error::other(
            "search is not prepared or catalog is catching up",
        ));
    } else {
        let delta = if path.with_extension("delta").exists() {
            Some(DeltaIndex::open_snapshot(
                &path.with_extension("delta"),
                base.generation(),
            )?)
        } else {
            None
        };
        browser.search_prepared(&path, &base, &query, delta.as_ref(), None, &|| false)?
    };
    println!(
        "records={} hits={} matched={} scanned={} query_ms={} total_ms={}",
        base.len(),
        result.0.len(),
        result.1.matched,
        result.1.scanned,
        result.1.wall_us as f64 / 1000.0,
        start.elapsed().as_millis()
    );
    Ok(())
}
