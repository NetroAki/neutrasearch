//! Scriptable Neutrasearch client. Opens the compact mmap index read-only and
//! never scans a filesystem. `--stdio` keeps one process alive for NDJSON RPC.
use anyhow::{bail, Context, Result};
use neutra_core::{CompactIndex, DeltaIndex, Query, SearchHit, SearchStats};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, Write};
use std::path::PathBuf;

#[derive(Deserialize)]
struct Request {
    query: String,
    limit: Option<usize>,
    metadata: Option<bool>,
}
#[derive(Serialize)]
struct Response {
    paths: Vec<String>,
    matched: u64,
    returned: usize,
    search_us: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    records: Option<Vec<Record>>,
}
#[derive(Serialize)]
struct Record {
    path: String,
    kind: String,
    size: u64,
    mtime: i64,
    fs: String,
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1).collect::<Vec<_>>();
    let mut index_path = None;
    let mut limit = 50usize;
    let mut json = false;
    let mut stdio = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--index" => {
                if i + 1 >= args.len() {
                    bail!("--index requires a path");
                }
                index_path = Some(PathBuf::from(args.remove(i + 1)));
                args.remove(i);
            }
            "--limit" => {
                if i + 1 >= args.len() {
                    bail!("--limit requires a number");
                }
                limit = args[i + 1]
                    .parse::<usize>()
                    .context("invalid --limit")?
                    .clamp(1, 1000);
                args.drain(i..=i + 1);
            }
            "--json" => {
                json = true;
                args.remove(i);
            }
            "--stdio" => {
                stdio = true;
                args.remove(i);
            }
            "--help" | "-h" => {
                println!("Usage: neutrasearch search QUERY [--index INDEX.nsx] [--limit N] [--json]\nInternal persistent mode: neutrasearch-query --index INDEX.nsx --stdio");
                return Ok(());
            }
            x if x.starts_with('-') => bail!("unknown option {x}"),
            _ => i += 1,
        }
    }
    let path = index_path.unwrap_or_else(default_index_path);
    let index = CompactIndex::open(&path)
        .with_context(|| format!("open compact index {}", path.display()))?;
    let delta = open_delta(&path, index.generation())?;
    if stdio {
        return serve(
            &index,
            delta,
            std::io::stdin().lock(),
            std::io::stdout().lock(),
        );
    }
    if args.is_empty() {
        bail!("query is required (or use --stdio)");
    }
    let request = Request {
        query: args.join(" "),
        limit: Some(limit),
        metadata: Some(json),
    };
    let response = run(&index, delta.as_ref(), request)?;
    if json {
        serde_json::to_writer(std::io::stdout().lock(), &response)?;
        println!();
    } else {
        for path in response.paths {
            println!("{path}");
        }
    }
    Ok(())
}
fn serve(
    index: &CompactIndex,
    mut delta: Option<DeltaIndex>,
    input: impl BufRead,
    mut output: impl Write,
) -> Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let request: Request = serde_json::from_str(&line)?;
        if let Some(delta) = &mut delta {
            delta.refresh()?;
        }
        match run(index, delta.as_ref(), request) {
            Ok(response) => serde_json::to_writer(&mut output, &response)?,
            Err(error) => {
                serde_json::to_writer(&mut output, &serde_json::json!({"error":error.to_string()}))?
            }
        };
        output.write_all(b"\n")?;
        output.flush()?;
    }
    Ok(())
}
fn run(index: &CompactIndex, delta: Option<&DeltaIndex>, request: Request) -> Result<Response> {
    let mut query = Query::parse(&request.query);
    query.limit = request.limit.unwrap_or(50).clamp(1, 1000);
    let (hits, stats) = match delta {
        Some(delta) => index.search_with_delta(&query, delta)?,
        None => index.search(&query)?,
    };
    Ok(response(hits, stats, request.metadata.unwrap_or(false)))
}
fn response(hits: Vec<SearchHit>, stats: SearchStats, metadata: bool) -> Response {
    let returned = hits.len();
    let paths = hits.iter().map(|h| h.record.path.to_string()).collect();
    let records = metadata.then(|| {
        hits.into_iter()
            .map(|h| Record {
                path: h.record.path.into(),
                kind: format!("{:?}", h.record.kind).to_ascii_lowercase(),
                size: h.record.size,
                mtime: h.record.mtime,
                fs: h.record.fs.label(),
            })
            .collect()
    });
    Response {
        paths,
        matched: stats.matched,
        returned,
        search_us: stats.wall_us,
        records,
    }
}
fn open_delta(base: &std::path::Path, generation: u64) -> Result<Option<DeltaIndex>> {
    let mut path = base.to_path_buf();
    path.set_extension("delta");
    if !path.is_file() {
        return Ok(None);
    }
    DeltaIndex::open_snapshot(&path, generation)
        .map(Some)
        .with_context(|| format!("open delta index {}", path.display()))
}

fn default_index_path() -> PathBuf {
    if let Some(path) =
        std::env::var_os("NEUTRASEARCH_INDEX").or_else(|| std::env::var_os("NEUTRA_INDEX"))
    {
        return path.into();
    }
    #[cfg(target_os = "windows")]
    {
        return std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join("Neutrasearch/index.nsx");
    }
    #[cfg(target_os = "macos")]
    {
        return std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join("Library/Caches/Neutrasearch/index.nsx");
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
            .unwrap_or_default()
            .join("neutrasearch/index.nsx")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neutra_core::{FileKind, FileRecord, FsKind};
    #[test]
    fn ndjson_api() {
        let path =
            std::env::temp_dir().join(format!("neutra-query-test-{}.nsx", std::process::id()));
        let records = vec![FileRecord {
            path: "/src/needle.rs".into(),
            size: 7,
            mtime: 1,
            mode: 0,
            kind: FileKind::File,
            fs: FsKind::Ext4,
            native_id: 1,
            native_parent: 2,
            source: 0,
        }];
        CompactIndex::build(&records, &path).unwrap();
        let index = CompactIndex::open(&path).unwrap();
        let mut output = Vec::new();
        serve(
            &index,
            None,
            "{\"query\":\"needle\",\"limit\":5}\n".as_bytes(),
            &mut output,
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(value["paths"][0], "/src/needle.rs");
        drop(index);
        std::fs::remove_file(path).unwrap();
    }
}
