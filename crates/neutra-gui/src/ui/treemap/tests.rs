use super::super::hierarchy::{FolderSummary, Hierarchy};
use super::*;

#[test]
fn empty_and_deep_ancestor_paths_are_safe() {
    let model = Hierarchy::empty();
    assert!(maps::map_blocks(&model, "/").is_empty());
    assert_eq!(
        ancestor_paths("/a/b"),
        vec!["/".to_owned(), "/a".to_owned(), "/a/b".to_owned()]
    );
    let mut model = model;
    model.folders.insert("/".into(), FolderSummary::default());
    assert!(maps::map_blocks(&model, "/").is_empty());
}
