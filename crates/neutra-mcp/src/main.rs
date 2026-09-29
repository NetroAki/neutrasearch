//! MCP stdio server exposing Neutrasearch's resident metadata index to agents.
//!
//! This replaces broad filename/path grep/find calls. It does not claim to
//! replace content grep: Neutrasearch intentionally indexes names + metadata.

mod policy;
mod store;
mod tools;

use anyhow::{bail, Result};
use policy::allowed_roots_from;
use tools::call_tool;
use serde_json::{json, Value};
use std::ffi::OsString;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use store::Store;

fn main() -> Result<()> {
     let index_path =
         configured_index_from(std::env::var_os("NEUTRASEARCH_INDEX"))?;
    let allowed_roots = allowed_roots_from(std::env::var_os("NEUTRASEARCH_MCP_ALLOWED_ROOTS"))?;
    serve(
        Store::open(index_path)?,
        &allowed_roots,
        &mut std::io::stdin().lock(),
        &mut std::io::stdout().lock(),
    )
}

fn serve<R: BufRead, W: Write>(
    mut index: Store,
    allowed_roots: &[PathBuf],
    r: &mut R,
    w: &mut W,
) -> Result<()> {
    for line in r.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let req: Value = serde_json::from_str(&line)?;
        let Some(id) = req.get("id").cloned() else {
            continue;
        };
        let method = req.get("method").and_then(Value::as_str).unwrap_or("");
        let result = match method {
            "initialize" => {
                json!({"protocolVersion":"2025-03-26","capabilities":{"tools":{}},"serverInfo":{"name":"neutrasearch","version":env!("CARGO_PKG_VERSION")}})
            }
            "tools/list" => tools::tools_list(),
            "tools/call" => call_tool(
                &mut index,
                allowed_roots,
                req.pointer("/params/name")
                    .and_then(Value::as_str)
                    .unwrap_or(""),
                req.pointer("/params/arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({})),
            ),
            "ping" => json!({}),
            _ => {
                write_json(
                    w,
                    &json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":format!("unknown method {method}")}}),
                )?;
                continue;
            }
        };
        write_json(w, &json!({"jsonrpc":"2.0","id":id,"result":result}))?;
    }
    Ok(())
}

fn write_json(w: &mut impl Write, v: &Value) -> Result<()> {
    serde_json::to_writer(&mut *w, v)?;
    w.write_all(b"\n")?;
    w.flush()?;
    Ok(())
}

 fn configured_index_from(configured: Option<OsString>) -> Result<PathBuf> {
     if let Some(path) = configured {
        if path.is_empty() {
            bail!("configured MCP index path must not be empty");
        }
        return Ok(PathBuf::from(path));
    }
    Ok(neutra_core::paths::resolve_index_path(None))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{path_is_allowed, portable_path_is_under, portable_path_text};
    use std::path::Path;
    use crate::store::Store;
    use neutra_core::{FileKind, FileRecord, FsKind, Index};
    use std::io::Cursor;

    fn empty_store() -> Store {
        Store::Legacy {
            path: PathBuf::from("test-index.bin"),
            index: Index::new(),
        }
    }

    #[test]
    fn mcp_lists_tools() {
        let mut input = Cursor::new(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\n");
        let mut out = Vec::new();
        serve(empty_store(), &[], &mut input, &mut out).unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["result"]["tools"][0]["name"], "neutra_search");
    }

    #[test]
    fn index_location_defaults_and_missing_file_fails() {
        assert_eq!(
             configured_index_from(None).unwrap(),
            neutra_core::paths::resolve_index_path(None)
        );
         assert!(configured_index_from(Some(OsString::new())).is_err());

        let missing = std::env::temp_dir().join(format!(
            "neutrasearch-mcp-missing-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        assert!(Store::open(missing).is_err());
    }

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
        let mut store = Store::Legacy {
            path: PathBuf::from("test-index.bin"),
            index,
        };
        let result = call_tool(
            &mut store,
            &[PathBuf::from(allowed_root)],
            "neutra_search",
            json!({"query":"needle", "limit":1}),
        );
        assert_eq!(result["structuredContent"]["paths"][0], allowed_file);
    }

     fn build_test_base(records: &[FileRecord], path: &std::path::Path) {
         let mut spill = neutra_core::SpillAccumulator::begin(path).unwrap();
         spill.push_batch(records.to_vec()).unwrap();
         neutra_core::CompactIndex::rebuild_streamed(spill.finish().unwrap(), path).unwrap();
     }

     #[test]
     fn directory_tool_serves_live_totals_and_rejects_legacy_stores() {
         let path =
             std::env::temp_dir().join(format!("neutra-mcp-dirsum-{}.nsx", std::process::id()));
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
        let result = call_tool(
            &mut store,
            &[],
            "neutra_directory",
            serde_json::json!({"path": "/docs"}),
        );
        assert_eq!(result["structuredContent"]["logical_bytes"], 10);
        assert_eq!(result["structuredContent"]["file_count"], 1);
        std::fs::remove_file(path).unwrap();

        let mut legacy = empty_store();
        let result = call_tool(&mut legacy, &[], "neutra_directory", json!({"path": "/"}));
        assert_eq!(result["isError"], true);
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
}
