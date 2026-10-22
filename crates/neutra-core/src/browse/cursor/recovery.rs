//! Recover a live WAL from a synchronized catalog while its service is stopped.
use crate::{BrowserIndex, CompactIndex, DeltaChange, DeltaIndex, FileRecord};
use std::{io, path::Path};

pub(in crate::browse) fn rebuild(index: &Path, output: &Path) -> io::Result<()> {
    let base = CompactIndex::open_fast(index)?;
    let _writer_lock = DeltaIndex::open(&index.with_extension("delta"), base.generation())?;
    let browser = BrowserIndex::open(index, base.generation())?;
    if !browser.covers_delta(index)? {
        return Err(io::Error::other("catalog does not cover the existing WAL"));
    }
    let transaction = browser
        .db
        .unchecked_transaction()
        .map_err(io::Error::other)?;
    let pinned: String = transaction
        .query_row("SELECT base_generation FROM metadata", [], |row| row.get(0))
        .map_err(io::Error::other)?;
    if pinned != base.generation().to_string() {
        return Err(io::Error::other(
            "recovery requires the original catalog base",
        ));
    }
    let dead = browser.tombstones()?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    drop(options.open(output)?);
    let mut delta = DeltaIndex::open_with_threshold(output, base.generation(), u64::MAX)?;
    let mut batch = Vec::with_capacity(4096);
    let mut removed = 0u64;
    let mut changed = 0u64;
    let mut id = 0u32;
    for block in 0..base.block_count() {
        for record in base.read_block(block as u32)? {
            if dead.contains(id) {
                batch.push(DeltaChange::Remove(record.path));
                removed += 1;
            }
            id += 1;
            if batch.len() == 4096 {
                delta.apply_batch(batch.drain(..))?;
            }
        }
    }
    if !batch.is_empty() {
        delta.apply_batch(batch.drain(..))?;
    }
    eprintln!("Recovered {removed} removed/replaced base addresses");
    let mut statement = transaction
        .prepare("SELECT record FROM entries WHERE record IS NOT NULL")
        .map_err(io::Error::other)?;
    let mut rows = statement.query([]).map_err(io::Error::other)?;
    while let Some(row) = rows.next().map_err(io::Error::other)? {
        let bytes: Vec<u8> = row.get(0).map_err(io::Error::other)?;
        let record: FileRecord = bincode::deserialize(&bytes).map_err(io::Error::other)?;
        batch.push(DeltaChange::Upsert(record));
        changed += 1;
        if batch.len() == 4096 {
            delta.apply_batch(batch.drain(..))?;
        }
    }
    if !batch.is_empty() {
        delta.apply_batch(batch.drain(..))?;
    }
    delta.sync()?;
    eprintln!(
        "Recovered {changed} live records, {} WAL bytes",
        delta.wal_bytes()
    );
    Ok(())
}
