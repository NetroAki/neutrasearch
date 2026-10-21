use super::*;

#[test]
fn allowed_roots_apply_before_query_limit() {
    let (denied, allowed_file, allowed_root) = if cfg!(target_os = "windows") {
        (
            r"C:\denied\needle.txt",
            r"C:\allowed\path\has-needle-here.txt",
            r"C:\allowed",
        )
    } else {
        (
            "/denied/needle.txt",
            "/allowed/path/has-needle-here.txt",
            "/allowed",
        )
    };
    let mut index = Index::new();
    for path in [denied, allowed_file] {
        index.push(FileRecord {
            path: path.into(),
            size: 1,
            mtime: 0,
            mode: 0,
            kind: FileKind::File,
            fs: FsKind::Ext4,
            native_id: 0,
            native_parent: 0,
            source: 0,
            disk: 0,
        });
    }
    let result = call_tool(
        &mut Store::Legacy {
            path: PathBuf::from("test-index.bin"),
            index,
        },
        &[PathBuf::from(allowed_root)],
        "neutra_search",
        json!({"query":"needle","limit":1}),
    );
    assert_eq!(result["structuredContent"]["paths"][0], allowed_file);
}

#[test]
fn windows_extended_roots_match_portable_ntfs_index_paths() {
    let root = portable_path_text(r"\\?\C:\Users\Alice");
    assert_eq!(root, "C:/Users/Alice");
    assert!(portable_path_is_under(
        "C:/Users/Alice/report.txt",
        &root,
        false
    ));
    assert!(portable_path_is_under(
        "c:/users/alice/report.txt",
        &root,
        false
    ));
    assert!(!portable_path_is_under(
        "C:/Users/Alicia/report.txt",
        &root,
        false
    ));
    assert_eq!(
        portable_path_text(r"\\?\UNC\server\share\team"),
        "//server/share/team"
    );
}

#[test]
fn portable_windows_paths_normalize_on_unix_without_rewriting_unix_names() {
    assert_eq!(
        portable_path_text(r"C:\Users\Alice\file.txt"),
        "C:/Users/Alice/file.txt"
    );
    assert_eq!(
        portable_path_text(r"\\server\share\file.txt"),
        "//server/share/file.txt"
    );
    #[cfg(unix)]
    assert_eq!(
        portable_path_text(r"/srv/allowed\secret/file.txt"),
        r"/srv/allowed\secret/file.txt"
    );
}

#[test]
fn allowed_roots_use_path_component_boundaries() {
    let (root, file, sibling, unsafe_path) = if cfg!(target_os = "windows") {
        (
            r"C:\home\a",
            r"C:\home\a\file.txt",
            r"C:\home\ab\file.txt",
            r"C:\home\a\..\secret",
        )
    } else {
        (
            "/home/a",
            "/home/a/file.txt",
            "/home/ab/file.txt",
            "/home/a/../secret",
        )
    };
    let roots = vec![PathBuf::from(root)];
    assert!(path_is_allowed(Path::new(file), &roots));
    assert!(path_is_allowed(Path::new(root), &roots));
    assert!(!path_is_allowed(Path::new(sibling), &roots));
    assert!(!path_is_allowed(Path::new(unsafe_path), &roots));
    assert!(!path_is_allowed(Path::new("relative/file.txt"), &roots));
}

#[cfg(unix)]
#[test]
fn unix_allowed_root_does_not_normalize_literal_backslashes() {
    let roots = vec![PathBuf::from("/srv/allowed")];
    assert!(path_is_allowed(Path::new("/srv/allowed/file.txt"), &roots));
    assert!(!path_is_allowed(
        Path::new("/srv/allowed\\secret/file.txt"),
        &roots
    ));
    assert!(!path_is_allowed(
        Path::new("/srv/allowed-sibling/file.txt"),
        &roots
    ));
}
