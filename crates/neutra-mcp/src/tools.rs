//! MCP tool surface: the advertised tool schemas and the dispatch that
//! serves them from the resident index store.

use crate::policy::path_is_allowed;
use crate::store::Store;
use neutra_core::Query;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub fn call_tool(index: &mut Store, allowed_roots: &[PathBuf], name: &str, args: Value) -> Value {
    match name {
        "neutra_search" => search::run(index, allowed_roots, args),
        "neutra_directory" => directory::run(index, allowed_roots, args),
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

mod directory;
mod search;
