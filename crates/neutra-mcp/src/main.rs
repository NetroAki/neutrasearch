//! MCP stdio server exposing Neutrasearch's resident metadata index to agents.
//!
//! This replaces broad filename/path grep/find calls. It does not claim to
//! replace content grep: Neutrasearch intentionally indexes names + metadata.

use anyhow::{Context, Result};
use neutra_core::{CompactIndex, DeltaIndex, Index, Query, SearchHit, SearchStats};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::PathBuf;

enum Store {
    Compact {
        base: CompactIndex,
        delta: Option<Box<DeltaIndex>>,
    },
    Legacy(Index),
}
impl Store {
    fn open() -> Result<Self> {
        let compact = compact_path();
        if looks_compact(&compact) {
            let base = CompactIndex::open(&compact)
                .with_context(|| format!("open {}", compact.display()))?;
            let delta_path = delta_path(&compact);
            let delta = if delta_path.is_file() {
                Some(Box::new(
                    DeltaIndex::open_snapshot(&delta_path, base.generation())
                        .with_context(|| format!("open {}", delta_path.display()))?,
                ))
            } else {
                None
            };
            return Ok(Self::Compact { base, delta });
        }
        let legacy = legacy_path();
        match std::fs::read(&legacy) {
            Ok(bytes) => Ok(Self::Legacy(
                Index::restore(&bytes).with_context(|| format!("decode {}", legacy.display()))?,
            )),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::Legacy(Index::new())),
            Err(e) => Err(e.into()),
        }
    }
    fn search(&self, q: &Query) -> Result<(Vec<SearchHit>, SearchStats)> {
        match self {
            Self::Compact {
                base,
                delta: Some(delta),
            } => Ok(base.search_with_delta(q, delta)?),
            Self::Compact { base, delta: None } => Ok(base.search(q)?),
            Self::Legacy(index) => Ok(index.search(q)),
        }
    }
    fn len(&self) -> u64 {
        match self {
            Self::Compact { base, .. } => base.len(),
            Self::Legacy(index) => index.len() as u64,
        }
    }
    fn kind(&self) -> &'static str {
        match self {
            Self::Compact { delta: Some(_), .. } => "compact-mmap+delta",
            Self::Compact { delta: None, .. } => "compact-mmap",
            Self::Legacy(_) => "legacy-resident",
        }
    }
    fn bytes(&self) -> u64 {
        match self {
            Self::Compact { base, delta } => {
                base.mapped_bytes() as u64 + delta.as_ref().map_or(0, |delta| delta.wal_bytes())
            }
            Self::Legacy(_) => std::fs::metadata(legacy_path())
                .map(|m| m.len())
                .unwrap_or(0),
        }
    }
}

fn main() -> Result<()> {
    serve(
        Store::open()?,
        &mut std::io::stdin().lock(),
        &mut std::io::stdout().lock(),
    )
}

fn serve<R: BufRead, W: Write>(index: Store, r: &mut R, w: &mut W) -> Result<()> {
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
                {"name":"neutra_status","description":"Report resident index size and cache path.","inputSchema":{"type":"object","properties":{}}}
            ]}),
            "tools/call" => call_tool(
                &index,
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

fn call_tool(index: &Store, name: &str, args: Value) -> Value {
    match name {
        "neutra_search" => {
            let raw = args.get("query").and_then(Value::as_str).unwrap_or("");
            let mut q = Query::parse(raw);
            q.limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(50)
                .clamp(1, 1000) as usize;
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
            let path = if matches!(index, Store::Compact { .. }) {
                compact_path()
            } else {
                legacy_path()
            };
            json!({"content":[{"type":"text","text":format!("{} indexed entries; {} store; {} bytes",index.len(),index.kind(),index.bytes())}],"structuredContent":{"records":index.len(),"store":index.kind(),"bytes":index.bytes(),"cache":path}})
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
fn legacy_path() -> PathBuf {
    if let Some(path) = configured_index() {
        return path;
    }
    #[cfg(target_os = "windows")]
    {
        return std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Neutrasearch/index.bin");
    }
    #[cfg(target_os = "macos")]
    {
        return std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Library/Caches/Neutrasearch/index.bin");
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
            .unwrap_or_else(|| PathBuf::from("."))
            .join("neutrasearch/index.bin")
    }
}
fn configured_index() -> Option<PathBuf> {
    std::env::var_os("NEUTRASEARCH_INDEX")
        .or_else(|| std::env::var_os("NEUTRA_INDEX"))
        .map(PathBuf::from)
}
fn compact_path() -> PathBuf {
    if let Some(path) = configured_index() {
        return path;
    }
    let mut path = legacy_path();
    path.set_extension("nsx");
    path
}
fn delta_path(base: &std::path::Path) -> PathBuf {
    let mut path = base.to_path_buf();
    path.set_extension("delta");
    path
}

fn looks_compact(path: &std::path::Path) -> bool {
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
    use std::io::Cursor;
    #[test]
    fn mcp_lists_tools() {
        let mut input = Cursor::new(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\n");
        let mut out = Vec::new();
        serve(Store::Legacy(Index::new()), &mut input, &mut out).unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["result"]["tools"][0]["name"], "neutra_search");
    }
}
