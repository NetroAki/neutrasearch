use super::*;

#[test]
fn catalog_exposes_missing_parent_without_expanding_its_subtree() {
    let fixture = Fixture::new("missing-parent");
    let base = fixture.build();
    BrowserIndex::ensure(&fixture.path, base.generation()).unwrap();
    let mut child = record("/d/orphan/leaf.dat", 9, FileKind::File);
    child.native_parent = 444;
    BrowserIndex::apply_changes(
        &fixture.path,
        base.generation(),
        &[DeltaChange::Upsert(child)],
    )
    .unwrap();
    let browser = BrowserIndex::open(&fixture.path, base.generation()).unwrap();
    let children = browser.directory_children("/d").unwrap();
    let parent = children
        .iter()
        .find(|record| record.path.as_ref() == "/d/orphan")
        .unwrap();
    assert_eq!(parent.kind, FileKind::Dir);
    assert_eq!(parent.native_id, 444);
    assert_eq!(parent.size, 0);
    assert_eq!(
        children
            .iter()
            .filter(|record| record.path.as_ref() == "/d/sub")
            .count(),
        1
    );
    BrowserIndex::apply_changes(
        &fixture.path,
        base.generation(),
        &[DeltaChange::RemovePrefix(parent.path.clone())],
    )
    .unwrap();
    assert!(browser
        .record_by_path("/d/orphan/leaf.dat")
        .unwrap()
        .is_none());
}
