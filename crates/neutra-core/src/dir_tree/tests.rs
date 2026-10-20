use super::*;
use crate::dir_summary::DirectorySummary;
use crate::{CompactIndex, FileRecord, FsKind};

#[test]
fn tree_preserves_drive_and_share_roots_without_self_children() {
    let records = vec![
        rec("C:/Docs/a.txt", 10, 0, FileKind::File, 0),
        rec("//server/share/team/b.txt", 20, 0, FileKind::File, 1),
        rec("/home/c.txt", 30, 0, FileKind::File, 2),
    ];
    let path = std::env::temp_dir().join(format!("neutra-tree-roots-{}.idx", std::process::id()));
    CompactIndex::build(&records, &path).unwrap();
    let index = CompactIndex::open_fast(&path).unwrap();
    let generation = index.generation();
    DirectorySummary::build(&records, &path, generation).unwrap();
    TreeSummary::ensure(&path, generation).unwrap();
    let tree = TreeSummary::open_for_compact(&path, generation).unwrap();
    let root = tree.listing("/").unwrap();
    let mut children: Vec<_> = root
        .subdirs
        .iter()
        .map(|child| crate::join_child_path("/", &child.name))
        .collect();
    children.sort();
    assert_eq!(children, ["//server/share", "/home", "C:/"]);
    assert_eq!(tree.listing("C:/").unwrap().total_count, 1);
    assert_eq!(tree.listing("//server/share").unwrap().total_count, 1);
    drop(index);
    for file in [
        path.clone(),
        directory_summary_path(&path),
        TreeSummary::path_for(&path),
    ] {
        std::fs::remove_file(file).unwrap();
    }
}

fn rec(path: &str, size: u64, disk: u64, kind: FileKind, source: u32) -> FileRecord {
    FileRecord {
        path: path.into(),
        size,
        mtime: 0,
        mode: 0,
        kind,
        fs: FsKind::Btrfs,
        native_id: size + 100,
        native_parent: 0,
        source,
        disk,
    }
}

#[test]
fn tree_listing_matches_the_streamed_listing_across_sources() {
    let records = vec![
        rec("/etc/a.conf", 10, 0, FileKind::File, 0),
        rec("/mnt/own.txt", 7, 0, FileKind::File, 0),
        rec("/home/u/doc/a.md", 3, 0, FileKind::File, 0),
        rec("/home/u/empty", 0, 0, FileKind::Dir, 0),
        rec("/mnt/keep/deep/song.wav", 1000, 600, FileKind::File, 1),
        rec("/mnt/keep/x.txt", 5, 0, FileKind::File, 1),
    ];
    let path = std::env::temp_dir().join(format!("neutra-tree-{}.idx", std::process::id()));
    CompactIndex::build(&records, &path).unwrap();
    let index = CompactIndex::open_fast(&path).unwrap();
    let generation = index.generation();
    DirectorySummary::build(&records, &path, generation).unwrap();
    TreeSummary::ensure(&path, generation).unwrap();
    assert!(TreeSummary::ensure(&path, generation + 1).is_err());
    let tree = TreeSummary::open_for_compact(&path, generation).unwrap();
    let shape = |listing: &DirListing| {
        let mut subs: Vec<_> = listing
            .subdirs
            .iter()
            .map(|s| (s.name.to_string(), s.size, s.logical, s.files))
            .collect();
        subs.sort();
        let mut files: Vec<_> = listing
            .files
            .iter()
            .map(|f| (f.name.to_string(), f.size, f.logical))
            .collect();
        files.sort();
        (
            listing.total_size,
            listing.total_logical,
            listing.total_count,
            subs,
            files,
        )
    };
    for dir in ["/", "/mnt", "/home", "/home/u", "/mnt/keep"] {
        let want = index.list_directory(dir, None, None).unwrap();
        let got = tree.listing(dir).unwrap();
        assert_eq!(shape(&got), shape(&want), "listing for {dir}");
    }
    assert!(tree.listing("/mnt/keep/deep/inner/x").is_none());
    drop(index);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(directory_summary_path(&path));
    let _ = std::fs::remove_file(TreeSummary::path_for(&path));
}
