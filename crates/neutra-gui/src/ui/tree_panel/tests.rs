use super::super::hierarchy::{DirChild, FolderSummary};
use super::*;

fn hierarchy() -> Hierarchy {
    let mut hierarchy = Hierarchy::empty();
    hierarchy.folders.insert(
        "/".into(),
        FolderSummary {
            size: 30,
            count: 3,
            children: vec![
                DirChild {
                    path: "/a".into(),
                    size: 20,
                    count: 2,
                },
                DirChild {
                    path: "/b".into(),
                    size: 10,
                    count: 1,
                },
            ],
            ..FolderSummary::default()
        },
    );
    hierarchy.folders.insert(
        "/a".into(),
        FolderSummary {
            size: 20,
            count: 2,
            children: vec![DirChild {
                path: "/a/nested".into(),
                size: 8,
                count: 1,
            }],
            direct_files: vec![TreeFile {
                path: "/a/readme.txt".into(),
                size: 12,
            }],
            ..FolderSummary::default()
        },
    );
    hierarchy
        .folders
        .insert("/b".into(), FolderSummary::default());
    hierarchy
}

fn key_action(
    hierarchy: &Hierarchy,
    current: &str,
    expanded: &BTreeSet<String>,
    key: egui::Key,
    focused_path: &str,
) -> Option<KeyAction> {
    let rows = flatten(hierarchy, current, expanded);
    let context = egui::Context::default();
    context.memory_mut(|memory| memory.request_focus(Id::new(("tree-row", focused_path))));
    let mut action = None;
    let _ = context.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(500.0, 300.0),
            )),
            events: vec![egui::Event::Key {
                key,
                physical_key: Some(key),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ui| {
            egui::CentralPanel::default().show_inside(ui, |ui| {
                let mut expanded = expanded.clone();
                action = keyboard(ui, &rows, current, &mut expanded);
            });
        },
    );
    action
}

fn focused_key(key: egui::Key, focused_path: &str) -> KeyAction {
    let hierarchy = hierarchy();
    let expanded = BTreeSet::from(["/".to_owned(), "/a".to_owned()]);
    key_action(&hierarchy, "/a", &expanded, key, focused_path)
        .expect("key should produce a tree action")
}

#[test]
fn flatten_keeps_folder_and_file_rows_in_visual_order() {
    let hierarchy = hierarchy();
    let expanded = BTreeSet::from(["/".to_owned(), "/a".to_owned()]);
    let rows = flatten(&hierarchy, "/a", &expanded);
    let paths: Vec<_> = rows.iter().map(|row| row.path).collect();
    assert_eq!(paths, ["/", "/a", "/a/nested", "/a/readme.txt", "/b"]);
    assert!(matches!(rows[3].kind, Kind::File(_)));
}

#[test]
fn down_from_folder_moves_focus_to_the_visible_file_row() {
    assert_eq!(
        focused_key(egui::Key::ArrowDown, "/a/nested"),
        KeyAction::Focus("/a/readme.txt".into(), 3)
    );
}

#[test]
fn enter_on_focused_file_opens_that_file() {
    assert_eq!(
        focused_key(egui::Key::Enter, "/a/readme.txt"),
        KeyAction::Open("/a/readme.txt".into())
    );
}

#[test]
fn right_expands_focused_folder_instead_of_the_open_folder() {
    assert_eq!(
        focused_key(egui::Key::ArrowRight, "/b"),
        KeyAction::Expand("/b".into())
    );
}

#[test]
fn right_on_expanded_leaf_does_not_jump_into_a_sibling_subtree() {
    let mut hierarchy = hierarchy();
    hierarchy.folders.get_mut("/a").unwrap().children.clear();
    hierarchy
        .folders
        .get_mut("/a")
        .unwrap()
        .direct_files
        .clear();
    hierarchy
        .folders
        .get_mut("/b")
        .unwrap()
        .children
        .push(DirChild {
            path: "/b/child".into(),
            size: 5,
            count: 1,
        });
    hierarchy
        .folders
        .insert("/b/child".into(), FolderSummary::default());
    let expanded = BTreeSet::from(["/".to_owned(), "/a".to_owned(), "/b".to_owned()]);
    let rows = flatten(&hierarchy, "/a", &expanded);
    let paths: Vec<_> = rows.iter().map(|row| row.path).collect();
    assert_eq!(paths, ["/", "/a", "/b", "/b/child"]);

    assert_eq!(
        key_action(&hierarchy, "/a", &expanded, egui::Key::ArrowRight, "/a"),
        None
    );
}
