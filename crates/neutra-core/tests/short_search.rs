use neutra_core::{
    BrowserIndex, CompactIndex, DeltaChange, FileKind, FileRecord, FsKind, MatchFields, Query,
    SortKey, SpillAccumulator,
};
use std::path::PathBuf;

fn record(path: &str, n: u64) -> FileRecord {
    FileRecord {
        path: path.into(),
        size: n,
        disk: if n.is_multiple_of(3) { u64::MAX } else { n * 2 },
        mtime: n as i64,
        mode: 0,
        kind: FileKind::File,
        fs: FsKind::Btrfs,
        native_id: n,
        native_parent: 0,
        source: 0,
    }
}

struct Fixture {
    root: PathBuf,
    path: PathBuf,
    base: CompactIndex,
    browser: BrowserIndex,
}
impl Fixture {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!("ns-short-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).unwrap();
        let path = root.join("index.nsx");
        let mut rows: Vec<_> = (0..40)
            .map(|n| record(&format!("/d/ab_{n:03}.txt"), n))
            .collect();
        rows.extend([
            record("/a", 50),
            record("/other/Äa", 51),
            record("/other/😀A", 52),
            record("/d/ab", 53),
            record("/d/zz", 54),
            record("/d/zz-1", 55),
            record("/d/zz-2", 56),
        ]);
        let mut spill = SpillAccumulator::begin(&path).unwrap();
        spill.push_batch(rows).unwrap();
        CompactIndex::rebuild_streamed(spill.finish().unwrap(), &path).unwrap();
        let base = CompactIndex::open_fast(&path).unwrap();
        BrowserIndex::ensure(&path, base.generation()).unwrap();
        let browser = BrowserIndex::open(&path, base.generation()).unwrap();
        Self {
            root,
            path,
            base,
            browser,
        }
    }
    fn finish(self) {
        drop(self.browser);
        drop(self.base);
        std::fs::remove_dir_all(self.root).unwrap();
    }
}

fn query(term: &str, sort: SortKey) -> Query {
    Query {
        terms: vec![term.into()],
        match_fields: MatchFields::Name,
        sort,
        limit: 3,
        ..Query::default()
    }
}

#[test]
fn short_queries_keep_exact_counts_scores_and_all_sort_orders() {
    let f = Fixture::new("orders");
    for sort in [
        SortKey::Relevance,
        SortKey::NameAsc,
        SortKey::NameDesc,
        SortKey::PathAsc,
        SortKey::PathDesc,
        SortKey::SizeAsc,
        SortKey::SizeDesc,
        SortKey::MtimeAsc,
        SortKey::MtimeDesc,
    ] {
        for term in ["a", "AB", "zz", "q1"] {
            let q = query(term, sort);
            let (hits, stats) = f
                .browser
                .search_short(&f.path, &q, None, &|| false)
                .unwrap()
                .unwrap();
            let (expected, expected_stats) = f.base.search(&q).unwrap();
            assert_eq!(stats.matched, expected_stats.matched, "{sort:?} / {term}");
            assert_eq!(stats.scanned, 0);
            assert_eq!(
                hits.iter()
                    .map(|h| (&h.record.path, h.score))
                    .collect::<Vec<_>>(),
                expected
                    .iter()
                    .map(|h| (&h.record.path, h.score))
                    .collect::<Vec<_>>()
            );
            if let Some(last) = hits.last() {
                let found = f
                    .browser
                    .search_short(&f.path, &q, Some(&last.record), &|| false)
                    .unwrap();
                if let Some((next, next_stats)) = found {
                    let expected = f
                        .base
                        .search_after_interruptible(&q, None, Some(&last.record), &|| false)
                        .unwrap();
                    assert_eq!(next_stats.matched, expected.1.matched);
                    assert_eq!(
                        next.iter()
                            .map(|h| (&h.record.path, h.score))
                            .collect::<Vec<_>>(),
                        expected
                            .0
                            .iter()
                            .map(|h| (&h.record.path, h.score))
                            .collect::<Vec<_>>()
                    );
                }
            }
        }
    }
    f.finish();
}

#[test]
fn live_names_and_prefix_deletion_update_short_totals_transactionally() {
    let f = Fixture::new("live");
    let changes = [
        DeltaChange::Remove("/a".into()),
        DeltaChange::Upsert(record("/d/ab", 75)),
        DeltaChange::Upsert(record("/a-new", 80)),
        DeltaChange::RemovePrefix("/d".into()),
    ];
    BrowserIndex::apply_changes(&f.path, f.base.generation(), &changes).unwrap();
    let q = query("a", SortKey::NameAsc);
    let (hits, stats) = f
        .browser
        .search_short(&f.path, &q, None, &|| false)
        .unwrap()
        .unwrap();
    assert_eq!(stats.matched, 3);
    assert_eq!(
        hits.iter()
            .map(|h| h.record.path.as_ref())
            .collect::<Vec<_>>(),
        ["/a-new", "/other/Äa", "/other/😀A"]
    );
    BrowserIndex::apply_changes(
        &f.path,
        f.base.generation(),
        &[DeltaChange::RemovePrefix("/d".into())],
    )
    .unwrap();
    assert_eq!(
        f.browser
            .search_short(&f.path, &q, None, &|| false)
            .unwrap()
            .unwrap()
            .1
            .matched,
        3
    );
    f.finish();
}

#[test]
fn unsupported_matching_options_and_unicode_casing_fall_back() {
    let f = Fixture::new("fallback");
    let q = query("a", SortKey::NameAsc);
    let variants = [
        Query {
            terms: vec!["i".into()],
            ..q.clone()
        },
        Query {
            terms: vec!["k".into()],
            ..q.clone()
        },
        Query {
            whole_word: true,
            ..q.clone()
        },
        Query {
            fold_accents: true,
            ..q.clone()
        },
        Query {
            case_sensitive: true,
            ..q.clone()
        },
        Query {
            match_fields: MatchFields::NameAndPath,
            ..q.clone()
        },
        Query {
            scope_roots: vec!["/d".into()],
            ..q.clone()
        },
        Query {
            exts: vec!["txt".into()],
            ..q.clone()
        },
    ];
    for q in variants {
        assert!(f
            .browser
            .search_short(&f.path, &q, None, &|| false)
            .unwrap()
            .is_none());
    }
    assert!(f.browser.search_short(&f.path, &q, None, &|| true).is_err());
    f.finish();
}
