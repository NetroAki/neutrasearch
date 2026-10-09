use neutra_core::{BrowserIndex, CompactIndex, Query, SortKey};

fn main() -> std::io::Result<()> {
    let path = std::env::args_os()
        .nth(1)
        .map(std::path::PathBuf::from)
        .expect("index path required");
    let index = CompactIndex::open_fast(&path)?;
    let started = std::time::Instant::now();
    BrowserIndex::ensure(&path, index.generation())?;
    eprintln!(
        "Prepared {} records in {:.2}s",
        index.len(),
        started.elapsed().as_secs_f64()
    );
    let browser = BrowserIndex::open(&path, index.generation())?;
    let maintenance = std::env::args().nth(2);
    if matches!(
        maintenance.as_deref(),
        Some("--sync-catalog" | "--checkpoint")
    ) {
        let mut writer =
            neutra_core::DeltaIndex::open(&path.with_extension("delta"), index.generation())?;
        BrowserIndex::apply_delta(&path, &writer)?;
        #[cfg(unix)]
        if maintenance.as_deref() == Some("--checkpoint") {
            writer.checkpoint()?;
        }
        BrowserIndex::mark_delta(&path, &writer)?;
    }
    let covered = browser.covers_delta(&path)?;
    eprintln!("Catalog covers durable delta: {covered}");
    let delta = if covered {
        None
    } else {
        neutra_core::DeltaIndex::open_snapshot(&path.with_extension("delta"), index.generation())
            .ok()
    };
    for sort in [
        SortKey::NameAsc,
        SortKey::NameDesc,
        SortKey::PathAsc,
        SortKey::PathDesc,
        SortKey::SizeAsc,
        SortKey::SizeDesc,
        SortKey::MtimeAsc,
        SortKey::MtimeDesc,
    ] {
        let query = Query {
            sort,
            limit: 1000,
            ..Query::default()
        };
        let (hits, stats) = browser.search(&index, &query, delta.as_ref())?;
        println!(
            "{sort:?}: {} rows, {:.3}s",
            hits.len(),
            stats.wall_us as f64 / 1_000_000.0
        );
    }
    Ok(())
}
