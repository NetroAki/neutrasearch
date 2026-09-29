//! Streamed builds carry every record at spill-chunk scale: the fixture
//! crosses chunk boundaries so multi-chunk merges run, then search hits and
//! directory totals prove the published base is complete.

use neutra_core::{CompactIndex, FileKind, FileRecord, FsKind, Query, SpillAccumulator};

fn fixture(count: usize) -> Vec<FileRecord> {
    let mut records = Vec::with_capacity(count);
    for id in 0..count {
        let dir = id % 37;
        records.push(FileRecord {
            path: format!("/t/dir{dir}/file{id:07}").into_boxed_str(),
            size: (id as u64).wrapping_mul(1_234_567),
            mtime: id as i64,
            mode: 0o644,
            kind: if id % 11 == 0 { FileKind::Dir } else { FileKind::File },
            fs: FsKind::Ext4,
            native_id: id as u64,
            native_parent: dir as u64,
            source: (id % 3) as u32,
            disk: id as u64,
        });
    }
    records
}

#[test]
fn streamed_build_indexes_every_record_at_scale() {
    let dir = std::env::temp_dir().join(format!("neutra-parity-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let records = fixture(1_000_001);
    let expected_files = records.iter().filter(|r| r.kind == FileKind::File).count() as u64;
    let expected_size: u64 = records.iter().filter(|r| r.kind == FileKind::File).map(|r| r.size).sum();
    let spilled = dir.join("streamed.nsx");
    let mut accumulator = SpillAccumulator::begin(&spilled).unwrap();
    for batch in records.chunks(100_000) {
        accumulator.push_batch(batch.to_vec()).unwrap();
    }
    let stats = CompactIndex::rebuild_streamed(accumulator.finish().unwrap(), &spilled).unwrap();
    assert_eq!(stats.records, 1_000_001);
    let index = CompactIndex::open(&spilled).unwrap();
    let query = Query::parse("file");
    let (hits, _) = index.search(&query).unwrap();
    assert!(!hits.is_empty());
    let listing = index.list_directory("/t", None, None).unwrap();
    assert_eq!(listing.total_count, expected_files);
    assert_eq!(listing.total_logical, expected_size);
    assert_eq!(listing.subdirs.len(), 37);
    let _ = std::fs::remove_dir_all(&dir);
}
