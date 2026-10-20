use super::*;

#[test]
fn mcp_lists_tools() {
    let mut input = Cursor::new(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\n");
    let mut output = Vec::new();
    crate::adapters::serve(empty_store(), &[], &mut input, &mut output).unwrap();
    let value: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["result"]["tools"][0]["name"], "neutra_search");
}

#[test]
fn index_location_defaults_and_missing_file_fails() {
    assert_eq!(
        crate::state::configured_index_from(None).unwrap(),
        neutra_core::paths::resolve_index_path(None)
    );
    assert!(crate::state::configured_index_from(Some(std::ffi::OsString::new())).is_err());
    let missing = std::env::temp_dir().join(format!(
        "neutrasearch-mcp-missing-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    assert!(Store::open(missing).is_err());
}
