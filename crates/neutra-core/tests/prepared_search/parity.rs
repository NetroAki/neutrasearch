use super::support::*;
use neutra_core::{FileKind, MatchFields, Query};

#[test]
fn prepared_results_match_compact_search_for_every_sort_and_query_option() {
    let rows = [
        record("/Work/Caf\u{e9}-notes.TXT", 90, 30, FileKind::File),
        record("/work/cafe-plan.txt", 30, 50, FileKind::File),
        record("/work/sub/cafe-report.md", 60, 40, FileKind::File),
        record("/work/run.sh", 10, 20, FileKind::File),
        record("/archive/cafe-notes.txt", 100, 10, FileKind::File),
        record("/work/folder", 0, 5, FileKind::Dir),
    ];
    let fixture = Fixture::build("parity", &rows);
    let templates = [
        Query {
            terms: vec!["cafe".into()],
            fold_accents: true,
            limit: 2,
            ..Query::default()
        },
        Query {
            terms: vec!["Caf\u{e9}".into()],
            case_sensitive: true,
            match_fields: MatchFields::Name,
            limit: 3,
            ..Query::default()
        },
        Query {
            terms: vec!["run".into()],
            whole_word: true,
            executable_only: true,
            limit: 4,
            ..Query::default()
        },
        Query {
            terms: vec!["work".into()],
            match_fields: MatchFields::Path,
            under: Some("/work".into()),
            limit: 5,
            ..Query::default()
        },
        Query {
            terms: vec!["txt".into()],
            exts: vec!["txt".into()],
            kinds: vec![FileKind::File],
            min_size: Some(20),
            max_size: Some(100),
            limit: 5,
            ..Query::default()
        },
        Query {
            terms: vec!["cafe".into()],
            scope_roots: vec!["/work".into()],
            exclude_roots: vec!["/work/sub".into()],
            limit: 5,
            ..Query::default()
        },
    ];
    for template in templates {
        for sort in ALL_SORTS {
            let query = Query {
                sort,
                ..template.clone()
            };
            let expected = fixture.base.search(&query).unwrap();
            let (cold, cold_stats) = fixture.prepared(&query, None, None);
            assert_eq!(paths(&cold), paths(&expected.0), "cold rows for {query:?}");
            assert_eq!(
                cold_stats.matched, expected.1.matched,
                "cold count for {query:?}"
            );
            let (warm, warm_stats) = fixture
                .browser
                .search_cached(&fixture.path, &query, None)
                .unwrap()
                .expect("prepared page stays cached");
            assert_eq!(paths(&warm), paths(&expected.0), "warm rows for {query:?}");
            assert_eq!(
                warm_stats.matched, expected.1.matched,
                "warm count for {query:?}"
            );
            assert_eq!(
                warm_stats.scanned, 0,
                "warm lookup must not scan for {query:?}"
            );
            let relimited = Query {
                limit: 1,
                ..query.clone()
            };
            let (short, short_stats) = fixture
                .browser
                .search_cached(&fixture.path, &relimited, None)
                .unwrap()
                .expect("signature ignores limit");
            assert_eq!(paths(&short), paths(&expected.0[..1.min(expected.0.len())]));
            assert_eq!(short_stats.matched, expected.1.matched);
        }
    }
    fixture.finish();
}
