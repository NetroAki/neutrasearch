//! MCP tool surface: the advertised tool schemas and the dispatch that
//! serves them from the resident index store.

use crate::policy::path_is_allowed;
use crate::store::Store;
use neutra_core::Query;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub fn call_tool(index: &mut Store, allowed_roots: &[PathBuf], name: &str, args: Value) -> Value {
    match name {
        "neutra_search" => {
            let raw = args.get("query").and_then(Value::as_str).unwrap_or("");
            let mut q = Query::parse(raw);
            q.limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(50)
                .clamp(1, 1000) as usize;
            q.scope_roots = allowed_roots
                .iter()
                .map(|root| root.to_string_lossy().into_owned())
                .collect();
            q.scope_case_sensitive = cfg!(not(any(target_os = "windows", target_os = "macos")));
            let metadata = args
                .get("metadata")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let (hits, stats) = match index.search(&q) {
                Ok(result) => result,
                Err(error) => {
                    return json!({"isError":true,"content":[{"type":"text","text":format!("index search failed: {error}")}]})
                }
            };
            // Defense in depth: trusted scopes are already applied by the query
            // engine before ranking and limiting.
            let hits = hits
                .into_iter()
                .filter(|hit| path_is_allowed(Path::new(hit.record.path.as_ref()), allowed_roots))
                .collect::<Vec<_>>();
            let returned = hits.len();
            let paths = hits
                .iter()
                .map(|h| h.record.path.to_string())
                .collect::<Vec<_>>();
            let text = if metadata {
                hits.iter()
                    .map(|h| {
                        format!(
                            "{}\t{:?}\t{}\t{}\t{}",
                            h.record.path,
                            h.record.kind,
                            h.record.size,
                            h.record.mtime,
                            h.record.fs.label()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                paths.join("\n")
            };
            let header = format!(
                "# matched={} returned={} search_us={}",
                stats.matched, returned, stats.wall_us
            );
            json!({"content":[{"type":"text","text":if text.is_empty(){header}else{format!("{header}\n{text}")}}],"structuredContent":{"paths":paths,"matched":stats.matched,"returned":returned,"search_us":stats.wall_us}})
        }
        "neutra_directory" => {
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
        "neutra_status" => {
            let basename = index
                .path()
                .file_name()
                .map(|name| name.to_string_lossy().into_owned());
            json!({"content":[{"type":"text","text":format!("{} indexed entries; {} store; {} bytes",index.len(),index.kind(),index.bytes())}],"structuredContent":{"records":index.len(),"store":index.kind(),"bytes":index.bytes(),"index_configured":true,"index_name":basename}})
        }
        _ => {
            json!({"isError":true,"content":[{"type":"text","text":format!("unknown tool {name}")}]})
        }
    }
}

pub fn tools_list() -> Value {
    json!({"tools":[
        {"name":"neutra_search","description":"Search the resident filename/path index without filesystem I/O.","inputSchema":{"type":"object","properties":{"query":{"type":"string","description":"Text + filters: ext:rs kind:file under:/src"},"limit":{"type":"integer","minimum":1,"maximum":1000,"default":50},"metadata":{"type":"boolean","default":false,"description":"Include kind/size/mtime/fs; false returns path lines only"}},"required":["query"]}},
        {"name":"neutra_status","description":"Report resident index status.","inputSchema":{"type":"object","properties":{}}},
        {"name":"neutra_directory","description":"Live totals for one indexed directory (logical bytes, file and directory counts, direct children).","inputSchema":{"type":"object","properties":{"path":{"type":"string","description":"Absolute directory path as indexed"},"source":{"type":"integer","default":0,"description":"Index source id (0 = local)"}},"required":["path"]}}
    ]})
}
