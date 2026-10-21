use super::*;

#[test]
fn directory_call_reopens_replaced_base_after_directory_only_session() {
    let path =
        std::env::temp_dir().join(format!("neutra-mcp-dir-reopen-{}.nsx", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let root = if cfg!(windows) { "C:/docs" } else { "/docs" };
    let record = |file: &str, size| {
        vec![
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
                path: format!("{root}/{file}").into(),
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
    let allowed = [PathBuf::from(root)];
    assert_eq!(
        call_tool(
            &mut store,
            &allowed,
            "neutra_directory",
            json!({"path":root})
        )["structuredContent"]["logical_bytes"],
        10
    );
    build_test_base(&record("new.txt", 42), &path);
    let refreshed = call_tool(
        &mut store,
        &allowed,
        "neutra_directory",
        json!({"path":root}),
    );
    assert_eq!(refreshed["structuredContent"]["logical_bytes"], 42);
    assert_eq!(
        refreshed["structuredContent"]["children"][0]["path"],
        format!("{root}/new.txt")
    );
    std::fs::remove_file(path).unwrap();
}
