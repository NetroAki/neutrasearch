use super::*;

fn file(path: &str) -> FileRecord {
    FileRecord {
        path: path.into(),
        size: 42,
        mtime: 0,
        mode: 0,
        kind: FileKind::File,
        fs: neutra_core::FsKind::Ntfs,
        native_id: 1,
        native_parent: 0,
        source: 0,
        disk: 0,
    }
}

fn listing() -> neutra_core::DirListing {
    use neutra_core::{DirFile, DirListing, DirSubdir};
    DirListing {
        files: vec![DirFile {
            name: "readme.md".into(),
            size: 12,
            logical: 12,
            kind: FileKind::File,
        }],
        subdirs: vec![DirSubdir {
            name: "archive".into(),
            size: 0,
            logical: 0,
            files: 0,
        }],
        total_size: 12,
        total_logical: 12,
        total_count: 1,
        files_truncated: false,
    }
}

#[test]
fn lazy_model_serves_fetched_dirs_and_evicts_beyond_budget() {
    let mut hierarchy = Hierarchy::empty();
    hierarchy.insert(
        "/docs".to_owned(),
        Hierarchy::folder_from_listing("/docs", listing()),
        true,
    );
    assert_eq!(hierarchy.folders["/docs"].size, 12);
    assert!(hierarchy.folders["/docs"].direct_files[0]
        .path
        .ends_with("readme.md"));
    assert_eq!(hierarchy.folders["/docs"].children[0].path, "/docs/archive");
    hierarchy.insert(
        "/old".to_owned(),
        Hierarchy::folder_from_listing("/old", listing()),
        false,
    );
    assert!(hierarchy.folders["/old"].direct_files.is_empty());
    hierarchy.bytes = Hierarchy::CACHE_BYTES + 1;
    hierarchy.evict(&["/docs".to_owned()]);
    assert!(!hierarchy.folders.contains_key("/old"));
    assert!(hierarchy.folders.contains_key("/docs"));
}

#[test]
fn hierarchy_connects_windows_drives_and_unc_shares_to_computer_root() {
    let sep = String::from(92 as char);
    let win = format!("C:{0}Users{0}Alex{0}report.txt", sep);
    let unc = format!("{0}{0}server{0}share{0}team{0}plan.txt", sep);
    let hierarchy = Hierarchy::from_records(&[file(&win), file(&unc)]);
    let root = hierarchy.folders.get("/").unwrap();
    assert!(root.children.iter().any(|child| child.path == "C:/"));
    assert!(root
        .children
        .iter()
        .any(|child| child.path == "//server/share"));
    assert_eq!((root.count, root.size), (2, 84));
    assert_eq!(
        (
            hierarchy.folders["C:/"].count,
            hierarchy.folders["C:/"].size
        ),
        (1, 42)
    );
    assert!(hierarchy.folders["C:/Users/Alex"].direct_files[0]
        .path
        .ends_with("report.txt"));
    assert!(hierarchy.folders["//server/share/team"].direct_files[0]
        .path
        .ends_with("plan.txt"));
}
