use super::*;

pub(super) fn run(index: &mut Store, args: Value) -> Value {
    let Some(path) = args.get("path").and_then(Value::as_str) else {
        return json!({"isError":true,"content":[{"type":"text","text":"neutra_directory requires \"path\""}]});
    };
    let source = args.get("source").and_then(Value::as_u64).unwrap_or(0) as u32;
    match index.directory_summary(source, path) {
        Ok(Some(entry)) => json!({
            "content":[{"type":"text","text":format!(
                "{}\t{} bytes\t{} files\t{} dirs",
                entry.path, entry.logical_bytes, entry.file_count, entry.directory_count
            )}],
            "structuredContent":{"path":entry.path,"logical_bytes":entry.logical_bytes,"file_count":entry.file_count,"directory_count":entry.directory_count,"children":entry.children.iter().map(|child| json!({"path":child.path,"kind":child.kind,"logical_bytes":child.logical_bytes})).collect::<Vec<_>>()}
        }),
        Ok(None) => {
            json!({"content":[{"type":"text","text":format!("no indexed directory summary for {path}")}],"structuredContent":{"path":path,"found":false}})
        }
        Err(error) => {
            json!({"isError":true,"content":[{"type":"text","text":format!("directory summary failed: {error}")}]})
        }
    }
}
