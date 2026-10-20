//! Times a delta fold into a staged base. Usage: fold_bench INDEX.nsx STAGED.nsx
use neutra_core::{CompactIndex, DeltaIndex};
use std::path::PathBuf;
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = PathBuf::from(args.next().ok_or("index path")?);
    let staged = PathBuf::from(args.next().ok_or("staged path")?);
    let base = CompactIndex::open_fast(&path)?;
    let delta = DeltaIndex::open_snapshot(&path.with_extension("delta"), base.generation())?;
    let t = Instant::now();
    let stats = CompactIndex::fold_delta(&base, &delta, &staged)?;
    println!(
        "fold: {:?} records={} bytes={}",
        t.elapsed(),
        stats.records,
        stats.bytes
    );
    Ok(())
}
