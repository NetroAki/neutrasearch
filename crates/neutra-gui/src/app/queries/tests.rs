use super::apply_kind_filter;

#[test]
fn kind_presets_map_to_engine_filters() {
    let mut query = neutra_core::Query::default();
    apply_kind_filter(&mut query, crate::ui::KindFilter::Audio);
    assert!(query.kinds.contains(&neutra_core::FileKind::File));
    assert!(query.exts.iter().any(|ext| ext == "mp3"));
    assert!(query.exts.iter().any(|ext| ext == "wav"));

    let mut programs = neutra_core::Query::default();
    apply_kind_filter(&mut programs, crate::ui::KindFilter::Programs);
    assert!(programs.executable_only);
    assert!(programs.exts.is_empty());

    let mut folders = neutra_core::Query::default();
    apply_kind_filter(&mut folders, crate::ui::KindFilter::Folders);
    assert_eq!(folders.kinds, vec![neutra_core::FileKind::Dir]);
}
