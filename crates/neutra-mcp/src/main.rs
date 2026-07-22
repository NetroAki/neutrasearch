//! MCP stdio server exposing Neutrasearch's resident metadata index to agents.
//!
//! This replaces broad filename/path grep/find calls. It does not claim to
//! replace content grep: Neutrasearch intentionally indexes names + metadata.

use anyhow::{bail, Context, Result};
use neutra_core::{CompactIndex, DeltaIndex, Index, Query, SearchHit, SearchStats};
use serde_json::{json, Value};
use std::ffi::OsString;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

enum Store {
    Compact {
        path: PathBuf,
        base: CompactIndex,
        delta: Option<Box<DeltaIndex>>,
    },
    Legacy {
        path: PathBuf,
        index: Index,
    },
}
impl Store {
    fn open(path: PathBuf) -> Result<Self> {
        if looks_compact(&path) {
            let (base, delta) = CompactIndex::open_with_delta_snapshot(&path)
                .with_context(|| format!("open {}", path.display()))?;
            return Ok(Self::Compact {
                path,
                base,
                delta: delta.map(Box::new),
            });
        }

        let bytes = std::fs::read(&path)
            .with_context(|| format!("read configured index {}", path.display()))?;
        let index = Index::restore(&bytes)
            .with_context(|| format!("decode configured index {}", path.display()))?;
        Ok(Self::Legacy { path, index })
    }
    fn search(&mut self, q: &Query) -> Result<(Vec<SearchHit>, SearchStats)> {
        let reopen = match self {
            Self::Compact {
                path,
                base,
                delta: Some(delta),
            } => {
                CompactIndex::generation_on_disk(path)? != base.generation()
                    || delta.refresh().is_err()
            }
            Self::Compact {
                path,
                base,
                delta: None,
            } => {
                CompactIndex::generation_on_disk(path)? != base.generation()
                    || delta_path(path).is_file()
            }
            Self::Legacy { .. } => false,
        };
        if reopen {
            let path = self.path().to_path_buf();
            *self = Self::open(path).context("reopen compact index after replacement")?;
        }
        match self {
            Self::Compact {
                base,
                delta: Some(delta),
                ..
            } => Ok(base.search_with_delta(q, delta)?),
            Self::Compact {
                base, delta: None, ..
            } => Ok(base.search(q)?),
            Self::Legacy { index, .. } => Ok(index.search(q)),
        }
    }
    fn path(&self) -> &Path {
        match self {
            Self::Compact { path, .. } | Self::Legacy { path, .. } => path,
        }
    }
    fn len(&self) -> u64 {
        match self {
            Self::Compact { base, .. } => base.len(),
            Self::Legacy { index, .. } => index.len() as u64,
        }
    }
    fn kind(&self) -> &'static str {
        match self {
            Self::Compact { delta: Some(_), .. } => "compact-mmap+delta",
            Self::Compact { delta: None, .. } => "compact-mmap",
            Self::Legacy { .. } => "legacy-resident",
        }
    }
    fn bytes(&self) -> u64 {
        match self {
            Self::Compact { base, delta, .. } => {
                base.mapped_bytes() as u64 + delta.as_ref().map_or(0, |delta| delta.wal_bytes())
            }
            Self::Legacy { path, .. } => std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
        }
    }
}

fn main() -> Result<()> {
    let index_path = configured_index_from(
        std::env::var_os("NEUTRASEARCH_INDEX"),
        std::env::var_os("NEUTRA_INDEX"),
    )?;
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
            "tools/list" => json!({"tools":[
                {"name":"neutra_search","description":"Search the resident filename/path index without filesystem I/O.","inputSchema":{"type":"object","properties":{"query":{"type":"string","description":"Text + filters: ext:rs kind:file under:/src"},"limit":{"type":"integer","minimum":1,"maximum":1000,"default":50},"metadata":{"type":"boolean","default":false,"description":"Include kind/size/mtime/fs; false returns path lines only"}},"required":["query"]}},
                {"name":"neutra_status","description":"Report resident index status.","inputSchema":{"type":"object","properties":{}}}
            ]}),
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

fn call_tool(index: &mut Store, allowed_roots: &[PathBuf], name: &str, args: Value) -> Value {
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
fn write_json(w: &mut impl Write, v: &Value) -> Result<()> {
    serde_json::to_writer(&mut *w, v)?;
    w.write_all(b"\n")?;
    w.flush()?;
    Ok(())
}

fn configured_index_from(primary: Option<OsString>, legacy: Option<OsString>) -> Result<PathBuf> {
    let Some(path) = primary.or(legacy) else {
        bail!("NEUTRASEARCH_INDEX (or legacy NEUTRA_INDEX) must be configured for MCP");
    };
    if path.is_empty() {
        bail!("configured MCP index path must not be empty");
    }
    Ok(PathBuf::from(path))
}

fn allowed_roots_from(value: Option<OsString>) -> Result<Vec<PathBuf>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let roots = std::env::split_paths(&value).collect::<Vec<_>>();
    if roots.is_empty() || roots.iter().any(|root| root.as_os_str().is_empty()) {
        bail!("NEUTRASEARCH_MCP_ALLOWED_ROOTS contains an empty path");
    }
    roots
        .into_iter()
        .map(|root| {
            if !safe_absolute_path(&root) {
                bail!("MCP allowed roots must be absolute and must not contain '..'");
            }
            std::fs::canonicalize(&root)
                .with_context(|| format!("resolve MCP allowed root {}", root.display()))
                .map(|root| PathBuf::from(portable_path(&root)))
        })
        .collect()
}

fn path_is_allowed(path: &Path, allowed_roots: &[PathBuf]) -> bool {
    safe_absolute_path(path)
        && (allowed_roots.is_empty()
            || allowed_roots.iter().any(|root| {
                portable_path_is_under(
                    &portable_path(path),
                    &portable_path(root),
                    cfg!(not(any(target_os = "windows", target_os = "macos"))),
                )
            }))
}

fn portable_path(path: &Path) -> String {
    portable_path_text(&path.to_string_lossy())
}

fn portable_path_text(path: &str) -> String {
    let replaced = path.replace('\\', "/");
    let mut normalized = if let Some(rest) = replaced.strip_prefix("//?/UNC/") {
        format!("//{rest}")
    } else if let Some(rest) = replaced
        .strip_prefix("//?/")
        .or_else(|| replaced.strip_prefix("//./"))
    {
        rest.to_owned()
    } else {
        replaced
    };
    let prefix_len = usize::from(normalized.starts_with("//")) * 2;
    while normalized[prefix_len..].contains("//") {
        let tail = normalized[prefix_len..].replace("//", "/");
        normalized.truncate(prefix_len);
        normalized.push_str(&tail);
    }
    if normalized.len() > 3 {
        normalized = normalized.trim_end_matches('/').to_owned();
    }
    normalized
}

fn portable_path_is_under(path: &str, root: &str, case_sensitive: bool) -> bool {
    let (path, root) = if case_sensitive {
        (path.to_owned(), root.to_owned())
    } else {
        (path.to_lowercase(), root.to_lowercase())
    };
    path.strip_prefix(&root)
        .is_some_and(|rest| rest.is_empty() || root.ends_with('/') || rest.starts_with('/'))
}

fn safe_absolute_path(path: &Path) -> bool {
    !path.to_string_lossy().contains('\0')
        && path.is_absolute()
        && !path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
}

fn delta_path(base: &Path) -> PathBuf {
    let mut path = base.to_path_buf();
    path.set_extension("delta");
    path
}

fn looks_compact(path: &Path) -> bool {
    use std::io::Read;
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut magic = [0u8; 8];
    file.read_exact(&mut magic).is_ok() && &magic == b"NEUTIDX1"
}

#[cfg(test)]
mod tests {
    use super::*;
    use neutra_core::{FileKind, FileRecord, FsKind};
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
    fn index_configuration_and_missing_file_fail() {
        assert!(configured_index_from(None, None).is_err());

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
