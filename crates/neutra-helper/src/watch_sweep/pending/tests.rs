use super::*;

fn remove(path: &str) -> DeltaChange {
    DeltaChange::Remove(path.into())
}

#[test]
fn newer_leaf_events_supersede_only_the_same_planned_path() {
    let mut plan = Pending::new(vec![remove("/a"), remove("/ab"), remove("/other")]);
    plan.supersede(remove("/a"));
    assert!(!plan.retry);
    assert_eq!(
        plan.batch(10).iter().map(path).collect::<Vec<_>>(),
        ["/ab", "/other"]
    );
}

#[test]
fn newer_subtree_removal_suppresses_old_children_but_not_siblings() {
    let mut plan = Pending::new(vec![remove("/a"), remove("/a/child"), remove("/ab")]);
    plan.supersede(DeltaChange::RemovePrefix("/a".into()));
    assert!(!plan.retry);
    assert_eq!(plan.batch(10).iter().map(path).collect::<Vec<_>>(), ["/ab"]);
}

#[test]
fn conflicting_folder_removal_requires_replanning_without_old_children() {
    let mut plan = Pending::new(vec![
        DeltaChange::RemovePrefix("/a".into()),
        remove("/a/old"),
        remove("/b"),
    ]);
    plan.supersede(remove("/a/newer"));
    assert!(plan.retry);
    assert_eq!(plan.batch(10).iter().map(path).collect::<Vec<_>>(), ["/b"]);
}

#[test]
fn parent_removal_conflicts_with_a_planned_nested_prefix() {
    let mut plan = Pending::new(vec![DeltaChange::RemovePrefix("/a/child".into())]);
    plan.supersede(DeltaChange::RemovePrefix("/a".into()));
    assert!(plan.retry);
    assert!(plan.is_empty());
}
