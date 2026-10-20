//! Times open, base-only search and base+delta search on a real index.
//! Usage: search_phases INDEX.nsx QUERY...
use neutra_core::{CompactIndex, DeltaIndex, Query};
use std::path::PathBuf;
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = PathBuf::from(args.next().ok_or("index path")?);
    let t = Instant::now();
    let base = CompactIndex::open_fast(&path)?;
    println!("open base: {:?}", t.elapsed());
    let t = Instant::now();
    let delta = if std::env::var_os("NO_DELTA").is_some() {
        None
    } else {
        match DeltaIndex::open_snapshot(&path.with_extension("delta"), base.generation()) {
            Ok(delta) => Some(delta),
            Err(error) => {
                println!("delta unavailable: {error}");
                None
            }
        }
    };
    println!("open delta: {:?} present={}", t.elapsed(), delta.is_some());
    for text in args {
        let mut query = Query::parse(&text);
        query.limit = 100;
        for (label, d) in [("base", None), ("base+delta", delta.as_ref())] {
            if d.is_none() && label != "base" {
                continue;
            }
            let t = Instant::now();
            let (hits, stats) = base.search_interruptible(&query, d, &|| false)?;
            println!(
                "{text:?} {label}: {:?} hits={} matched={}",
                t.elapsed(),
                hits.len(),
                stats.matched
            );
        }
    }
    Ok(())
}
