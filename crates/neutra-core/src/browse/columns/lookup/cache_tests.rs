use super::*;
use crate::{BrowserIndex, DeltaChange, FileKind, FsKind};

fn record(path: String) -> FileRecord {
    FileRecord {
        path: path.into(),
        size: 1,
        disk: 1,
        mtime: 1,
        mode: 0,
        kind: FileKind::File,
        fs: FsKind::Ext4,
        source: 0,
        native_id: 1,
        native_parent: 1,
    }
}

#[test]
fn nearby_watcher_lookups_reuse_blocks_but_observe_live_changes() {
    let temp = std::env::temp_dir().join(format!("neutra-lookup-cache-{}", std::process::id()));
    std::fs::create_dir(&temp).unwrap();
    let path = temp.join("cached.nsx");
    let records: Vec<_> = (0..4096)
        .map(|id| record(format!("/root/{id:05}.txt")))
        .collect();
    let built = CompactIndex::build(&records, &path).unwrap();
    BrowserIndex::ensure(&path, built.generation).unwrap();
    let browser = BrowserIndex::open(&path, built.generation).unwrap();
    let queries = [
        "/root/02000.txt",
        "/root/02001.txt",
        "/root/02000.txt-missing",
        "/root/é",
    ];
    for query in queries {
        let mut uncached = BlockReader::new(&browser.base);
        let expected =
            crate::browse::catalog::find_record(&browser.db, &mut uncached, query).unwrap();
        assert_eq!(
            bincode::serialize(&browser.record_by_path(query).unwrap()).unwrap(),
            bincode::serialize(&expected).unwrap()
        );
    }
    let before = decode_count();
    for query in queries {
        browser.record_by_path(query).unwrap();
    }
    assert_eq!(
        decode_count(),
        before,
        "warm lookups must not decode blocks again"
    );
    let mut updated = records[2000].clone();
    updated.size = 999;
    BrowserIndex::apply_changes(
        &path,
        built.generation,
        &[
            DeltaChange::Upsert(updated.clone()),
            DeltaChange::Remove(records[2001].path.clone()),
        ],
    )
    .unwrap();
    assert_eq!(
        bincode::serialize(&browser.record_by_path(&updated.path).unwrap()).unwrap(),
        bincode::serialize(&Some(updated.clone())).unwrap()
    );
    assert!(browser
        .record_by_path(&records[2001].path)
        .unwrap()
        .is_none());
    drop(browser);
    crate::browse::updates::writer_cache::clear_for_test();
    std::fs::remove_dir_all(temp).unwrap();
}

#[test]
fn decoded_block_cache_evicts_and_skips_oversized_blocks() {
    let mut cache = DecodedBlocks::new();
    for block in 0..=DECODED_BLOCK_LIMIT as u32 {
        cache
            .get_or_load(block, || Ok(vec![record(format!("/{block}"))]))
            .unwrap();
    }
    assert_eq!(cache.entries.len(), DECODED_BLOCK_LIMIT);
    assert!(!cache.entries.iter().any(|entry| entry.0 == 0));
    let bytes = cache.bytes;
    cache
        .get_or_load(9999, || {
            Ok(vec![record("x".repeat(DECODED_BYTE_LIMIT + 1))])
        })
        .unwrap();
    assert_eq!(cache.bytes, bytes);
    assert!(!cache.entries.iter().any(|entry| entry.0 == 9999));
}
