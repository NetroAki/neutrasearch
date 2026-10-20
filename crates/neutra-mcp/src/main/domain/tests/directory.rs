use super::*;

#[test]
fn directory_tool_serves_live_totals_and_rejects_legacy_stores() {
    let path = std::env::temp_dir().join(format!("neutra-mcp-dirsum-{}.nsx", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let records = vec![
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
            path: "/docs/a.txt".into(),
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
    let result = call_tool(&mut store, &[], "neutra_directory", json!({"path":"/docs"}));
    assert_eq!(result["structuredContent"]["logical_bytes"], 10);
    assert_eq!(result["structuredContent"]["file_count"], 1);
    std::fs::remove_file(path).unwrap();
    assert_eq!(
        call_tool(
            &mut empty_store(),
            &[],
            "neutra_directory",
            json!({"path":"/"})
        )["isError"],
        true
    );
}
