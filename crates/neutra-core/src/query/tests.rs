use super::*;

fn rec(path: &str, size: u64) -> FileRecord {
    FileRecord {
        path: path.into(),
        size,
        mtime: 0,
        mode: 0,
        kind: if path.ends_with('/') {
            FileKind::Dir
        } else {
            FileKind::File
        },
        fs: FsKind::Ext4,
        native_id: 0,
        native_parent: 0,
        source: 0,
        disk: 0,
    }
}

#[test]
fn parses_filters() {
    let q = Query::parse("main ext:rs,toml size:>1k kind:file under:/src");
    assert_eq!(q.terms, vec!["main"]);
    assert_eq!(q.exts, vec!["rs", "toml"]);
    assert_eq!(q.min_size, Some(1024));
    assert_eq!(q.kinds, vec![FileKind::File]);
    assert_eq!(q.under.as_deref(), Some("/src"));
}

#[test]
fn under_filter_respects_path_component_boundaries() {
    let q = Query::parse("under:/home/a");
    assert!(q.passes_filters(&rec("/home/a/file.txt", 1)));
    assert!(q.passes_filters(&rec("/home/a", 1)));
    assert!(!q.passes_filters(&rec("/home/ab/file.txt", 1)));

    let windows = Query::parse(r"under:C:\Users\A");
    assert!(windows.passes_filters(&rec(r"c:\Users\A\file.txt", 1)));
    assert!(!windows.passes_filters(&rec(r"c:\Users\AB\file.txt", 1)));
}

#[test]
fn trusted_scope_roots_are_orred_with_component_boundaries() {
    let mut q = Query::parse("");
    q.scope_roots = vec!["/allowed/a".into(), "/allowed/b".into()];
    assert!(q.passes_filters(&rec("/allowed/a/file.txt", 1)));
    assert!(q.passes_filters(&rec("/allowed/b/file.txt", 1)));
    assert!(!q.passes_filters(&rec("/allowed/ab/file.txt", 1)));
    assert!(!q.passes_filters(&rec("/denied/file.txt", 1)));
    assert!(!q.passes_filters(&rec("/allowed/a/../secret.txt", 1)));
}

#[test]
fn trusted_windows_scope_accepts_native_or_portable_separators() {
    let mut query = Query::parse("");
    query.scope_roots = vec![r"C:\Users\Alex".into()];
    assert!(query.passes_filters(&rec("C:/Users/Alex/report.txt", 1)));
    assert!(query.passes_filters(&rec(r"c:\users\alex\report.txt", 1)));
    assert!(!query.passes_filters(&rec("C:/Users/Alexander/report.txt", 1)));
}

#[test]
fn trusted_scope_can_enforce_case_sensitive_host_semantics() {
    let mut query = Query::parse("");
    query.scope_roots = vec!["/Users/Alice".into()];
    assert!(query.passes_filters(&rec("/users/alice/file.txt", 1)));
    query.scope_case_sensitive = true;
    assert!(query.passes_filters(&rec("/Users/Alice/file.txt", 1)));
    assert!(!query.passes_filters(&rec("/users/alice/file.txt", 1)));
}

#[test]
fn scores_name_over_path() {
    let q = Query::parse("config");
    let name_hit = rec("/home/u/projects/config.rs", 10);
    let path_hit = rec("/home/u/config/main.rs", 10);
    let s_name = q.score(&name_hit).unwrap();
    let s_path = q.score(&path_hit).unwrap();
    assert!(s_name > s_path);
    assert!(Query::parse("zzz").score(&name_hit).is_none());
}

#[test]
fn size_ranges() {
    let q = Query::parse("size:1M..2M");
    assert_eq!(q.min_size, Some(1 << 20));
    assert_eq!(q.max_size, Some(2 << 20));
    assert!(q.passes_filters(&rec("/a/b", 1_500_000)));
    assert!(!q.passes_filters(&rec("/a/b", 3 << 20)));
}

#[test]
fn quoted_terms() {
    let q = Query::parse("\"my doc\" ext:txt");
    assert_eq!(q.terms, vec!["my doc"]);
    assert_eq!(q.exts, vec!["txt"]);
}

#[test]
fn regex_mode_matches_like_terms() {
    let mut q = Query::parse("");
    q.regex = Some(r"inv[o0]ice.*\.pdf$".into());
    let hit = rec("/home/u/Invoice-2026.pdf", 1);
    let miss = rec("/home/u/invoice.txt", 1);
    let matcher = q.matcher().unwrap();
    assert!(matcher.score(&hit).is_some());
    assert!(matcher.score(&miss).is_none());

    q.regex = Some("([".into());
    assert!(q.matcher().is_err());
}

#[test]
fn case_sensitive_terms_reject_other_case() {
    let mut q = Query::parse("Report");
    let upper = rec("/home/u/Report.docx", 1);
    let lower = rec("/home/u/report.docx", 1);
    assert!(q.score(&upper).is_some());
    assert!(q.score(&lower).is_some());
    q.case_sensitive = true;
    assert!(q.score(&upper).is_some());
    assert!(q.score(&lower).is_none());
}

#[test]
fn match_fields_restrict_where_terms_match() {
    let mut q = Query::parse("projects");
    let name_hit = rec("/home/u/Projects", 1);
    let path_hit = rec("/home/u/projects/readme.md", 1);
    assert!(q.score(&name_hit).is_some());
    assert!(q.score(&path_hit).is_some());
    q.match_fields = MatchFields::Name;
    assert!(q.score(&name_hit).is_some());
    assert!(q.score(&path_hit).is_none());
    q.match_fields = MatchFields::Path;
    assert!(q.score(&path_hit).is_some());
}

#[test]
fn executable_and_whole_word_options_narrow_matches() {
    let mut q = Query::parse("call");
    q.whole_word = true;
    // Name-only default in the GUI aside, the engine honors the flag:
    // "calling" is not a whole-word hit for "call".
    assert!(q
        .matcher()
        .unwrap()
        .score(&rec("/m/the calling.wav", 1))
        .is_none());
    assert!(q
        .matcher()
        .unwrap()
        .score(&rec("/m/the call.wav", 1))
        .is_some());
    let mut exe = Query::parse("");
    exe.executable_only = true;
    assert!(!exe.passes_filters(&rec("/usr/bin/tool", 1)));
    assert!(exe.passes_filters(&rec("/opt/app/setup.exe", 1)));
}
