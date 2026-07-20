use neutra_core::{CompactIndex, FileKind, FileRecord, FsKind, Query};
use std::path::PathBuf;
use std::time::Instant;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let count = std::env::args()
        .nth(1)
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(1_000_000);
    let output = std::env::args()
        .nth(2)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("neutra-compact-benchmark.idx"));
    let started = Instant::now();
    let mut records = Vec::with_capacity(count);
    for i in 0..count {
        let mixed = (i as u64).wrapping_mul(0x9e3779b97f4a7c15).rotate_left(17);
        let ext = match i % 7 {
            0 => "rs",
            1 => "toml",
            2 => "jpg",
            3 => "txt",
            4 => "so",
            5 => "json",
            _ => "bin",
        };
        records.push(FileRecord {
            path: format!(
                "/volume/{:04x}/project-{:08x}/needle{:08x}-{:016x}.{ext}",
                i % 4096,
                (mixed >> 32) as u32,
                i,
                mixed
            )
            .into_boxed_str(),
            size: mixed & 0x00ff_ffff,
            mtime: 1_700_000_000 + i as i64 % 1_000_000,
            mode: 0o100644,
            kind: FileKind::File,
            fs: FsKind::Btrfs,
            native_id: i as u64,
            native_parent: (i % 4096) as u64,
            source: 0,
        });
    }
    let generated_ms = started.elapsed().as_millis();
    let built = CompactIndex::build(&records, &output)?;
    drop(records);
    let index = CompactIndex::open(&output)?;
    let query = Query::parse(&format!("needle{:08x}", count.saturating_sub(1)));
    let mut best = u64::MAX;
    let mut returned = 0;
    for _ in 0..10 {
        let start = Instant::now();
        let (hits, _) = index.search(&query)?;
        best = best.min(start.elapsed().as_micros() as u64);
        returned = hits.len();
    }
    println!("records={} generated_ms={} build_ms={} bytes={} bytes_per_record={:.2} blocks={} trigrams={} query_best_us={} returned={} output={}",count,generated_ms,built.wall_ms,built.bytes,built.bytes as f64/count.max(1)as f64,built.blocks,built.trigrams,best,returned,output.display());
    Ok(())
}
