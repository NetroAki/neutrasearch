use super::*;
use crate::{DeltaChange, FileKind, FsKind};

fn named(path: &str, size: u64) -> FileRecord {
    FileRecord {
        path: path.into(),
        size,
        mtime: size as i64,
        mode: 0,
        kind: FileKind::File,
        fs: FsKind::Btrfs,
        native_id: size,
        native_parent: 0,
        source: 0,
        disk: 0,
    }
}

const NAMES: [&str; 7] = [
    "/m/Caf\u{e9}/Calling Stem.WAV",
    "/m/cafe/calling.stem.wav",
    "/m/\u{212a}elvin/kick.wav",
    "/m/stems/calling_01.flac",
    "/m/stems/recalling.txt",
    "/m/x/CALL ME.mp3",
    "/m/y/na\u{ef}ve call.doc",
];

const QUERIES: [&str; 10] = [
    "calling",
    "call",
    "cafe",
    "kelvin",
    "stem ext:wav",
    "stems",
    "naive",
    "ing",
    "zzz",
    "size:>5 call",
];

/// The borrowed-view scan must agree with scoring every owned record, for
/// case, accent, whole-word and filter variations, with a live delta.
#[test]
fn scan_agrees_with_scoring_every_record_across_match_options() {
    let dir = std::env::temp_dir().join(format!("neutra-oracle-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("index.nsx");
    let mut records: Vec<_> = NAMES
        .iter()
        .enumerate()
        .map(|(i, name)| named(name, i as u64 + 1))
        .collect();
    for i in 0..100 {
        records.push(named(&format!("/filler/{i}/f{i}.bin"), 1000 + i));
    }
    records.sort_by(|a, b| a.path.cmp(&b.path));
    CompactIndex::build(&records, &path).unwrap();
    let index = CompactIndex::open(&path).unwrap();
    let mut delta = DeltaIndex::open(&path.with_extension("delta"), index.generation()).unwrap();
    delta
        .apply(DeltaChange::Remove("/m/x/CALL ME.mp3".into()))
        .unwrap();
    delta
        .apply(DeltaChange::Upsert(named("/m/new/Calling Live.wav", 77)))
        .unwrap();
    let mut pool = records.clone();
    pool.retain(|r| &*r.path != "/m/x/CALL ME.mp3");
    pool.push(named("/m/new/Calling Live.wav", 77));
    for text in QUERIES {
        for (case, word, accents) in [
            (false, false, false),
            (true, false, false),
            (false, true, false),
            (false, false, true),
        ] {
            let mut query = Query::parse(text);
            query.limit = 0;
            query.case_sensitive = case;
            query.whole_word = word;
            query.fold_accents = accents;
            let matcher = query.matcher().unwrap();
            let mut want: Vec<_> = pool
                .iter()
                .filter(|r| query.passes_filters(r))
                .filter_map(|r| matcher.score(r).map(|s| (s, r.path.to_string())))
                .collect();
            let (hits, stats) = index.search_with_delta(&query, &delta).unwrap();
            let mut got: Vec<_> = hits
                .into_iter()
                .map(|h| (h.score, h.record.path.to_string()))
                .collect();
            want.sort();
            got.sort();
            assert_eq!(
                got, want,
                "{text} case={case} word={word} accents={accents}"
            );
            assert_eq!(stats.matched as usize, want.len());
        }
    }
    drop(delta);
    drop(index);
    std::fs::remove_dir_all(dir).unwrap();
}
