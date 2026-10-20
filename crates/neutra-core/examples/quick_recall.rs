//! Measure eight-result filename recall. Usage: quick_recall INDEX.nsx TERM...
use neutra_core::{BrowserIndex, CompactIndex, DeltaIndex, MatchFields, Query};
use std::{io, path::PathBuf, time::Instant};

fn main() -> io::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = PathBuf::from(
        args.next()
            .ok_or_else(|| io::Error::other("index path required"))?,
    );
    let start = Instant::now();
    let base = CompactIndex::open_fast(&path)?;
    let browser = BrowserIndex::open(&path, base.generation())?;
    println!(
        "records={} open_ms={:.3}",
        base.len(),
        start.elapsed().as_secs_f64() * 1000.0
    );
    let mut delta = None;
    for term in args {
        let query = Query {
            terms: vec![term.clone()],
            match_fields: MatchFields::Name,
            limit: 8,
            ..Query::default()
        };
        let start = Instant::now();
        let fast = browser.search_quick(&path, &query, &|| false)?;
        let mut indexed = fast.is_some();
        let hits = match fast {
            Some(hits) => hits,
            None => {
                if delta.is_none() && path.with_extension("delta").exists() {
                    delta = Some(
                        DeltaIndex::open_search_snapshot(
                            &path.with_extension("delta"),
                            base.generation(),
                        )
                        .or_else(|_| {
                            DeltaIndex::open_snapshot(
                                &path.with_extension("delta"),
                                base.generation(),
                            )
                        })?,
                    );
                }
                match browser.search_quick_with_delta(&path, &query, delta.as_mut(), &|| false)? {
                    Some(hits) => {
                        indexed = true;
                        hits
                    }
                    None => {
                        base.search_interruptible(&query, delta.as_ref(), &|| false)?
                            .0
                    }
                }
            }
        };
        println!(
            "term={term:?} indexed={indexed} hits={} ms={:.3}",
            hits.len(),
            start.elapsed().as_secs_f64() * 1000.0
        );
    }
    Ok(())
}
