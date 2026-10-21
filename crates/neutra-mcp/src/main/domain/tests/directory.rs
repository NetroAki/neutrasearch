use super::*;

#[test]
fn directory_tool_serves_live_totals_and_rejects_legacy_stores() {
    let path = std::env::temp_dir().join(format!("neutra-mcp-dirsum-{}.nsx", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let root = if cfg!(windows) { "C:/docs" } else { "/docs" };
    let file = format!("{root}/a.txt");
    let sibling = if cfg!(windows) {
        "C:/docs-sibling"
    } else {
        "/docs-sibling"
    };
    let allowed_root = PathBuf::from(root);
    let records = vec![
        FileRecord {
            path: root.into(),
            size: 0,
            mtime: 0,
            mode: 0,
            kind: FileKind::Dir,
            fs: FsKind::Ext4,
            native_id: 1,
            native_parent: 0,
            source: 0,
            disk: 0,
        },
        FileRecord {
            path: file.into(),
            size: 10,
            mtime: 0,
            mode: 0,
            kind: FileKind::File,
            fs: FsKind::Ext4,
            native_id: 2,
            native_parent: 1,
            source: 0,
            disk: 0,
        },
    ];
    build_test_base(&records, &path);
    let mut store = Store::open(path.clone()).unwrap();
    let result = call_tool(
        &mut store,
        std::slice::from_ref(&allowed_root),
        "neutra_directory",
        json!({"path":root}),
    );
    assert_eq!(result["structuredContent"]["logical_bytes"], 10);
    assert_eq!(result["structuredContent"]["file_count"], 1);
    let denied = call_tool(
        &mut store,
        std::slice::from_ref(&allowed_root),
        "neutra_directory",
        json!({"path":sibling}),
    );
    assert_eq!(denied["isError"], true);
    std::fs::remove_file(path).unwrap();
    assert_eq!(
        call_tool(
            &mut empty_store(),
            &[allowed_root],
            "neutra_directory",
            json!({"path":root})
        )["isError"],
        true
    );
}

#[cfg(unix)]
#[test]
fn directory_response_never_reconstructs_literal_backslash_sibling() {
    let path = std::env::temp_dir().join(format!(
        "neutra-mcp-dir-backslash-{}.nsx",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    let records = vec![
        FileRecord {
            path: "/srv/allowed".into(),
            size: 0,
            mtime: 0,
            mode: 0,
            kind: FileKind::Dir,
            fs: FsKind::Ext4,
            native_id: 1,
            native_parent: 0,
            source: 0,
            disk: 0,
        },
        FileRecord {
            path: "/srv/allowed\\secret/file.txt".into(),
            size: 77,
            mtime: 0,
            mode: 0,
            kind: FileKind::File,
            fs: FsKind::Ext4,
            native_id: 2,
            native_parent: 1,
            source: 0,
            disk: 0,
        },
    ];
    build_test_base(&records, &path);
    let mut store = Store::open(path.clone()).unwrap();
    let result = call_tool(
        &mut store,
        &[PathBuf::from("/srv/allowed")],
        "neutra_directory",
        json!({"path":"/srv/allowed"}),
    );
    assert_ne!(result["isError"], true);
    let children = result["structuredContent"]["children"].as_array().unwrap();
    assert!(children
        .iter()
        .all(|child| !child["path"].as_str().unwrap().contains("secret")));
    assert_eq!(result["structuredContent"]["file_count"], 0);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn directory_call_reopens_replaced_base_after_directory_only_session() {
    let path =
        std::env::temp_dir().join(format!("neutra-mcp-dir-reopen-{}.nsx", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let record = |file: &str, size| {
        vec![
            FileRecord {
                path: "/docs".into(),
                size: 0,
                mtime: 0,
                mode: 0,
                kind: FileKind::Dir,
                fs: FsKind::Ext4,
                native_id: 1,
                native_parent: 0,
                source: 0,
                disk: 0,
            },
            FileRecord {
                path: format!("/docs/{file}").into(),
                size,
                mtime: 0,
                mode: 0,
                kind: FileKind::File,
                fs: FsKind::Ext4,
                native_id: 2,
                native_parent: 1,
                source: 0,
                disk: 0,
            },
        ]
    };
    build_test_base(&record("old.txt", 10), &path);
    let mut store = Store::open(path.clone()).unwrap();
    let allowed = [PathBuf::from("/docs")];
    assert_eq!(
        call_tool(
            &mut store,
            &allowed,
            "neutra_directory",
            json!({"path":"/docs"})
        )["structuredContent"]["logical_bytes"],
        10
    );
    build_test_base(&record("new.txt", 42), &path);
    let refreshed = call_tool(
        &mut store,
        &allowed,
        "neutra_directory",
        json!({"path":"/docs"}),
    );
    assert_eq!(refreshed["structuredContent"]["logical_bytes"], 42);
    assert_eq!(
        refreshed["structuredContent"]["children"][0]["path"],
        "/docs/new.txt"
    );
    std::fs::remove_file(path).unwrap();
}
