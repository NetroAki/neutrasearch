use super::*;
use crate::{DeltaChange, FileKind, FileRecord, FsKind, SortKey};

fn record(path: &str, size: u64, disk: u64, mtime: i64) -> FileRecord {
    FileRecord {
        path: path.into(),
        size,
        disk,
        mtime,
        mode: 0,
        kind: FileKind::File,
        fs: FsKind::Btrfs,
        native_id: size,
        native_parent: 0,
        source: 0,
    }
}

#[test]
fn catalog_recovery_restores_changed_added_removed_and_unchanged_records() {
    let path = std::env::temp_dir().join(format!("ns-catalog-recovery-{}.nsx", std::process::id()));
    let output = path.with_extension("recovered.delta");
    let _ = std::fs::remove_file(&output);
    CompactIndex::build(
        &[
            record("/keep", 1, 1, 1),
            record("/replace", 2, 2, 2),
            record("/gone", 3, 3, 3),
            record("/tail", 4, 4, 4),
        ],
        &path,
    )
    .unwrap();
    let base = CompactIndex::open_fast(&path).unwrap();
    let generation = base.generation();
    BrowserIndex::ensure(&path, generation).unwrap();
    BrowserIndex::apply_changes(
        &path,
        generation,
        &[
            DeltaChange::Remove("/gone".into()),
            DeltaChange::Upsert(record("/replace", 99, 99, 99)),
            DeltaChange::Upsert(record("/added", 5, 5, 5)),
        ],
    )
    .unwrap();
    let mut lost = DeltaIndex::open(&path.with_extension("delta"), generation).unwrap();
    lost.reset(generation).unwrap();
    BrowserIndex::mark_delta(&path, &lost).unwrap();
    drop(lost);
    BrowserIndex::recover_delta(&path, &output).unwrap();
    let recovered = DeltaIndex::open_snapshot(&output, generation).unwrap();
    let (hits, stats) = base
        .search_with_delta(&Query::default(), &recovered)
        .unwrap();
    assert_eq!(stats.matched, 4);
    assert!(!hits.iter().any(|hit| &*hit.record.path == "/gone"));
    assert_eq!(recovered.upsert_for("/replace").unwrap().unwrap().size, 99);
    assert!(hits.iter().any(|hit| &*hit.record.path == "/keep"));
    assert!(hits.iter().any(|hit| &*hit.record.path == "/tail"));
    assert!(hits.iter().any(|hit| &*hit.record.path == "/added"));
    let mut pending = DeltaIndex::open(&path.with_extension("delta"), generation).unwrap();
    pending.apply(DeltaChange::Remove("/keep".into())).unwrap();
    pending.sync().unwrap();
    drop(pending);
    let another = path.with_extension("unsafe.delta");
    assert!(BrowserIndex::recover_delta(&path, &another).is_err());
    assert!(!another.exists());
}

#[test]
fn hierarchy_cursor_handles_root_unicode_prefix_boundaries_and_overflow() {
    let path = std::env::temp_dir().join(format!("ns-hierarchy-{}.nsx", std::process::id()));
    let records = vec![
        record("/a", 1, 1, 1),
        record("/ab", 2, 2, 2),
        record("/a/child", 3, 3, 3),
        record("/😀", 4, 4, 4),
        record("/𐀀", 5, 5, 5),
        record("/folder", 6, 6, 6),
        record("/folder/one", 7, 7, 7),
        record("/folder/two", 8, 8, 8),
    ];
    CompactIndex::build(&records, &path).unwrap();
    let base = CompactIndex::open_fast(&path).unwrap();
    BrowserIndex::ensure(&path, base.generation()).unwrap();
    let browser = BrowserIndex::open(&path, base.generation()).unwrap();

    let root = browser.directory_children("/").unwrap();
    assert_eq!(
        root.iter().map(|r| r.path.as_ref()).collect::<Vec<_>>(),
        ["/a", "/ab", "/folder", "/𐀀", "/😀"]
    );
    let ordered = browser.hierarchy_path_order().unwrap();
    let expected_direct: Vec<_> = ordered
        .iter()
        .filter(|p| !p[1..].contains('/'))
        .map(String::as_str)
        .collect();
    assert_eq!(
        root.iter().map(|r| r.path.as_ref()).collect::<Vec<_>>(),
        expected_direct
    );
    let subtree = browser.subtree_records("/folder", 10).unwrap().unwrap();
    assert_eq!(
        subtree.iter().map(|r| r.path.as_ref()).collect::<Vec<_>>(),
        ["/folder/one", "/folder/two"]
    );
    assert!(browser.subtree_records("/folder", 1).unwrap().is_none());

    let delta_path = path.with_extension("delta");
    let mut delta = DeltaIndex::open(&delta_path, base.generation()).unwrap();
    let added = record("/新", 9, 9, 9);
    let changes = [
        DeltaChange::Upsert(added.clone()),
        DeltaChange::Remove("/ab".into()),
    ];
    delta.apply_batch(changes.clone()).unwrap();
    BrowserIndex::apply_changes(&path, base.generation(), &changes).unwrap();
    let root = browser.directory_children("/").unwrap();
    let ordered = browser.hierarchy_path_order().unwrap();
    let expected_direct: Vec<_> = ordered
        .iter()
        .filter(|p| !p[1..].contains('/'))
        .map(String::as_str)
        .collect();
    assert_eq!(
        root.iter().map(|r| r.path.as_ref()).collect::<Vec<_>>(),
        expected_direct
    );
    drop(delta);

    drop(browser);
    drop(base);
    for file in [
        &path,
        &BrowserIndex::path_for(&path),
        &super::updates::base_path(&path),
        &path.with_extension("nsx.browse.lock"),
        &path.with_extension("delta"),
    ] {
        let _ = std::fs::remove_file(file);
    }
}

#[test]
fn every_order_and_page_matches_the_engine_with_scopes_and_live_changes() {
    let path = std::env::temp_dir().join(format!("ns-browse-{}.nsx", std::process::id()));
    let records = vec![
        record("/a/Z.wav", 500, 20, 20),
        record("/a/a.txt", 50, 50, 30),
        record("/a/A.txt", 50, 50, 30),
        record("/b/a.txt", 80, 80, 10),
        record("/a/sparse.vhd", 2_000_000, 10, 50),
        record("/ab/other.txt", 100, 100, 99),
    ];
    CompactIndex::build(&records, &path).unwrap();
    let base = CompactIndex::open_fast(&path).unwrap();
    BrowserIndex::ensure(&path, base.generation()).unwrap();
    let browser = BrowserIndex::open(&path, base.generation()).unwrap();
    let delta_path = path.with_extension("delta");
    let mut delta = DeltaIndex::open(&delta_path, base.generation()).unwrap();
    delta.apply(DeltaChange::Remove("/a/a.txt".into())).unwrap();
    delta
        .apply(DeltaChange::Upsert(record("/a/new.txt", 90, 90, 999)))
        .unwrap();
    let paths = |hits: &[SearchHit]| {
        hits.iter()
            .map(|hit| hit.record.path.to_string())
            .collect::<Vec<_>>()
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
        for scope in [vec![], vec!["/a".into()]] {
            for exts in [vec![], vec!["txt".into()]] {
                let mut query = Query {
                    sort,
                    scope_roots: scope.clone(),
                    scope_case_sensitive: true,
                    exts,
                    limit: 100,
                    ..Query::default()
                };
                let expected = base.search_with_delta(&query, &delta).unwrap().0;
                query.limit = 2;
                for offset in [0, 2, 4, 6] {
                    let actual = browser
                        .search_page(&base, &query, Some(&delta), offset)
                        .unwrap()
                        .0;
                    assert_eq!(
                        paths(&actual),
                        paths(
                            &expected
                                .iter()
                                .skip(offset)
                                .take(2)
                                .cloned()
                                .collect::<Vec<_>>()
                        ),
                        "{sort:?}"
                    );
                }
            }
        }
    }
    assert!(BrowserIndex::open(&path, base.generation() + 1).is_err());
    drop(browser);
    drop(delta);
    drop(base);
    for file in [
        &path,
        &delta_path,
        &BrowserIndex::path_for(&path),
        &path.with_extension("nsx.browse.lock"),
        &super::updates::base_path(&path),
    ] {
        let _ = std::fs::remove_file(file);
    }
}

#[test]
fn durable_changes_and_keyset_pages_survive_base_replacement() {
    let path = std::env::temp_dir().join(format!("ns-browse-pinned-{}.nsx", std::process::id()));
    let records = vec![
        record("/a/one.txt", 1, 1, 1),
        record("/a/two.txt", 2, 2, 2),
        record("/a/three.txt", 3, 3, 3),
    ];
    CompactIndex::build(&records, &path).unwrap();
    let base = CompactIndex::open_fast(&path).unwrap();
    let old = base.generation();
    BrowserIndex::ensure(&path, old).unwrap();
    // Reuse the unlocked preparation file after a previous build or crash.
    BrowserIndex::ensure(&path, old).unwrap();
    let changes = [
        DeltaChange::Remove("/a/two.txt".into()),
        DeltaChange::Upsert(record("/a/new.txt", 4, 4, 4)),
    ];
    BrowserIndex::apply_changes(&path, old, &changes).unwrap();
    let current = vec![
        records[0].clone(),
        records[2].clone(),
        record("/a/new.txt", 4, 4, 4),
    ];
    let staged = path.with_extension("staged");
    CompactIndex::build(&current, &staged).unwrap();
    let new = CompactIndex::open_fast(&staged).unwrap();
    let generation = new.generation();
    drop(new);
    CompactIndex::publish(&staged, &path).unwrap();
    BrowserIndex::advance_generation(&path, old, generation).unwrap();
    let current_base = CompactIndex::open_fast(&path).unwrap();
    let browser = BrowserIndex::open(&path, generation).unwrap();
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
            limit: 1,
            ..Query::default()
        };
        let expected = current_base
            .search(&Query {
                limit: 100,
                ..query.clone()
            })
            .unwrap()
            .0;
        let mut anchor = None;
        for hit in expected {
            let page = browser
                .search_after(&current_base, &query, None, anchor.as_ref())
                .unwrap()
                .0;
            assert_eq!(page.len(), 1);
            assert_eq!(page[0].record.path, hit.record.path, "{sort:?}");
            anchor = Some(page[0].record.clone());
        }
        assert!(browser
            .search_after(&current_base, &query, None, anchor.as_ref())
            .unwrap()
            .0
            .is_empty());
    }
    drop(browser);
    drop(current_base);
    drop(base);
    for file in [
        &path,
        &staged,
        &BrowserIndex::path_for(&path),
        &super::updates::base_path(&path),
        &path.with_extension("nsx.browse.lock"),
        &path.with_extension("nsx.browse-wal"),
        &path.with_extension("nsx.browse-shm"),
    ] {
        let _ = std::fs::remove_file(file);
    }
}
