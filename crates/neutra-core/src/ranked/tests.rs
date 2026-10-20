use super::*;
use crate::FsKind;

fn rec(path: &str, size: u64, mtime: i64, kind: crate::FileKind) -> FileRecord {
    FileRecord {
        path: path.into(),
        size,
        mtime,
        mode: 0,
        kind,
        fs: FsKind::Btrfs,
        native_id: size + 1,
        native_parent: 0,
        source: 0,
        disk: size,
    }
}

#[test]
fn leaders_match_the_full_search_for_both_orderings_and_filters() {
    let records = vec![
        rec("/a/old.txt", 5, 100, crate::FileKind::File),
        rec("/a/new.wav", 900, 900, crate::FileKind::File),
        rec("/a/mid.txt", 70, 500, crate::FileKind::File),
        rec("/a/dir", 0, 700, crate::FileKind::Dir),
        rec("/b/big.bin", 5000, 300, crate::FileKind::File),
    ];
    let path = std::env::temp_dir().join(format!("neutra-rank-{}.idx", std::process::id()));
    CompactIndex::build(&records, &path).unwrap();
    let index = CompactIndex::open_fast(&path).unwrap();
    let generation = index.generation();
    assert!(!RankedLists::is_current(&path, generation));
    RankedLists::ensure(&path, generation).unwrap();
    assert!(RankedLists::is_current(&path, generation));
    let ranked = RankedLists::open_for_compact(&path, generation).unwrap();
    let names = |hits: &[SearchHit]| {
        hits.iter()
            .map(|h| h.record.path.to_string())
            .collect::<Vec<_>>()
    };
    for sort in [SortKey::MtimeDesc, SortKey::SizeDesc] {
        for kinds in [vec![], vec![crate::FileKind::File]] {
            let query = Query {
                sort,
                limit: 3,
                kinds,
                ..Query::default()
            };
            let want = index.search(&query).unwrap().0;
            let got = ranked.search(&query, None).unwrap().expect("answerable").0;
            assert_eq!(names(&got), names(&want), "{sort:?}");
        }
    }
    let typed = Query {
        terms: vec!["old".into()],
        sort: SortKey::MtimeDesc,
        limit: 3,
        ..Query::default()
    };
    assert!(ranked.search(&typed, None).unwrap().is_none());
    let by_name = Query {
        sort: SortKey::NameAsc,
        limit: 3,
        ..Query::default()
    };
    assert!(ranked.search(&by_name, None).unwrap().is_none());
    drop(index);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(RankedLists::path_for(&path));
}

#[test]
fn a_saved_file_leads_the_newest_listing_and_a_removed_one_leaves_it() {
    let records = vec![
        rec("/a/one.txt", 5, 100, crate::FileKind::File),
        rec("/a/two.txt", 6, 200, crate::FileKind::File),
        rec("/a/three.txt", 7, 300, crate::FileKind::File),
    ];
    let path = std::env::temp_dir().join(format!("neutra-rank-delta-{}.idx", std::process::id()));
    CompactIndex::build(&records, &path).unwrap();
    let generation = CompactIndex::open_fast(&path).unwrap().generation();
    RankedLists::ensure(&path, generation).unwrap();
    let ranked = RankedLists::open_for_compact(&path, generation).unwrap();
    let delta_path = path.with_extension("delta");
    let mut delta = DeltaIndex::open(&delta_path, generation).unwrap();
    delta
        .apply(crate::DeltaChange::Upsert(rec(
            "/a/saved.txt",
            9,
            999,
            crate::FileKind::File,
        )))
        .unwrap();
    delta
        .apply(crate::DeltaChange::Remove("/a/three.txt".into()))
        .unwrap();
    let query = Query {
        sort: SortKey::MtimeDesc,
        limit: 10,
        ..Query::default()
    };
    let hits = ranked
        .search(&query, Some(&delta))
        .unwrap()
        .expect("answerable")
        .0;
    let paths: Vec<_> = hits.iter().map(|h| h.record.path.to_string()).collect();
    assert_eq!(paths, ["/a/saved.txt", "/a/two.txt", "/a/one.txt"]);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&delta_path);
    let _ = std::fs::remove_file(RankedLists::path_for(&path));
}

#[test]
fn corrupt_future_timestamps_do_not_lead_the_newest_listing() {
    let records = vec![
        rec("/a/real.txt", 5, 1_790_000_000, crate::FileKind::File),
        rec("/a/bogus.txt", 5, 4_500_000_000, crate::FileKind::File),
    ];
    let path = std::env::temp_dir().join(format!("neutra-rank-future-{}.idx", std::process::id()));
    CompactIndex::build(&records, &path).unwrap();
    let index = CompactIndex::open_fast(&path).unwrap();
    let query = Query {
        sort: SortKey::MtimeDesc,
        limit: 5,
        ..Query::default()
    };
    assert_eq!(
        &*index.search(&query).unwrap().0[0].record.path,
        "/a/real.txt"
    );
    drop(index);
    let _ = std::fs::remove_file(&path);
}
