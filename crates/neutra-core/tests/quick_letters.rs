use neutra_core::{
    BrowserIndex, CompactIndex, DeltaChange, DeltaIndex, FileKind, FileRecord, FsKind, MatchFields,
    Query, SpillAccumulator,
};

fn record(name: &str, mtime: i64) -> FileRecord {
    FileRecord {
        path: format!("/root/{name}").into(),
        size: 1,
        disk: 1,
        mtime,
        mode: 0,
        kind: FileKind::File,
        fs: FsKind::Btrfs,
        native_id: 0,
        native_parent: 0,
        source: 0,
    }
}

#[test]
fn quick_letters_preserve_unicode_ranking_with_live_changes() {
    let root = std::env::temp_dir().join(format!("ns-quick-letters-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("index.nsx");
    let mut spill = SpillAccumulator::begin(&path).unwrap();
    spill
        .push_batch(
            [
                "i", "I", "İ", "ix", "İx", "iy", "İy", "iZ", "k", "K", "K", "kx", "Kx", "ky", "Ky",
                "kZ", "tail-i-k",
            ]
            .iter()
            .enumerate()
            .map(|(i, name)| record(name, i as i64))
            .collect(),
        )
        .unwrap();
    CompactIndex::rebuild_streamed(spill.finish().unwrap(), &path).unwrap();
    let base = CompactIndex::open_fast(&path).unwrap();
    BrowserIndex::ensure(&path, base.generation()).unwrap();
    let browser = BrowserIndex::open(&path, base.generation()).unwrap();
    let mut delta = DeltaIndex::open(&path.with_extension("delta"), base.generation()).unwrap();
    BrowserIndex::mark_delta(&path, &delta).unwrap();
    for live in [false, true] {
        if live {
            delta
                .apply_batch([
                    DeltaChange::Remove("/root/İ".into()),
                    DeltaChange::Remove("/root/Kx".into()),
                    DeltaChange::Upsert(record("İnew", 100)),
                    DeltaChange::Upsert(record("Knew", 101)),
                ])
                .unwrap();
            delta.sync().unwrap();
        }
        for term in ["i", "I", "k", "K"] {
            for limit in [1, 2, 3, 5, 7] {
                let query = Query {
                    terms: vec![term.into()],
                    match_fields: MatchFields::Name,
                    limit,
                    ..Query::default()
                };
                let quick = browser
                    .search_quick_with_delta(&path, &query, Some(&mut delta), &|| false)
                    .unwrap()
                    .unwrap();
                let full = base.search_with_delta(&query, &delta).unwrap().0;
                let ranked = |hits: Vec<neutra_core::SearchHit>| {
                    hits.into_iter()
                        .map(|hit| (hit.score, hit.record.path))
                        .collect::<Vec<_>>()
                };
                assert_eq!(ranked(quick), ranked(full), "{term}/{limit}/live={live}");
            }
        }
    }
    drop(delta);
    drop(browser);
    drop(base);
    std::fs::remove_dir_all(root).unwrap();
}
