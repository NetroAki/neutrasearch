use super::*;

#[test]
fn indexed_space_preserves_windows_drive_and_share_scopes() {
    let search = Query {
        scope_roots: vec![r"C:\Music".into(), "//server/share/Photos".into()],
        scope_case_sensitive: false,
        ..Query::default()
    };
    assert_eq!(
        scoped_query(&search, "/", false).unwrap().scope_roots,
        search.scope_roots
    );
    assert_eq!(
        scoped_query(&search, "c:/Music/Stems", false)
            .unwrap()
            .scope_roots,
        vec!["c:/Music/Stems"]
    );
    assert!(scoped_query(&search, "D:/Music", false).is_none());
    assert!(scoped_query(&search, "//server/other", false).is_none());
}

#[test]
fn maps_preserve_search_filters_and_intersect_navigation_scope() {
    let mut search = Query::parse("call ext:wav under:/music/live");
    search.scope_roots = vec!["/music".into(), "/photos".into()];
    search.scope_case_sensitive = true;
    search.whole_word = true;
    search.case_sensitive = true;
    search.fold_accents = true;
    search.exclude_roots = vec!["/music/live/private".into()];
    let map = scoped_query(&search, "/music/live", false).unwrap();
    assert_eq!(map.scope_roots, vec!["/music/live"]);
    let mut expected = search.clone();
    expected.scope_roots = map.scope_roots.clone();
    expected.kinds = vec![FileKind::File, FileKind::Symlink];
    expected.sort = SortKey::SizeDesc;
    expected.limit = 2048;
    assert_eq!(map, expected);
    assert_eq!(
        scoped_query(&search, "/", true).unwrap().scope_roots,
        search.scope_roots
    );
    assert!(scoped_query(&search, "/musical", false).is_none());
    assert!(scoped_query(&search, "/Music", false).is_none());
    assert!(scoped_query(&Query::default(), "/", false).is_none());
}

#[test]
fn maps_do_not_broaden_folder_only_filters() {
    let search = Query {
        kinds: vec![FileKind::Dir],
        scope_roots: vec!["/".into()],
        ..Query::default()
    };
    assert!(scoped_query(&search, "/", false).is_none());
    let search = Query {
        kinds: vec![FileKind::Dir, FileKind::File],
        ..search
    };
    assert_eq!(
        scoped_query(&search, "/", true).unwrap().kinds,
        vec![FileKind::File]
    );
}
