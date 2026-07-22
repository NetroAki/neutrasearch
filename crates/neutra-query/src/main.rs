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
    #[serde(default)]
    scope_roots: Vec<String>,
    #[serde(default)]
    scope_case_sensitive: Option<bool>,
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
    let mut json_paths = false;
    let mut stdio = false;
    let mut scope_roots = Vec::new();
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
            "--scope" => {
                if i + 1 >= args.len() {
                    bail!("--scope requires an absolute path");
                }
                let scope = PathBuf::from(args.remove(i + 1));
                if !scope.is_absolute() {
                    bail!("--scope requires an absolute path");
                }
                scope_roots.push(scope.to_string_lossy().into_owned());
                args.remove(i);
            }
            "--json" => {
                json = true;
                args.remove(i);
            }
            "--json-paths" => {
                json_paths = true;
                args.remove(i);
            }
            "--stdio" => {
                stdio = true;
                args.remove(i);
            }
            "--help" | "-h" => {
                println!("Usage: neutrasearch search QUERY [--index INDEX.nsx] [--scope ROOT] [--limit N] [--json|--json-paths]\nInternal persistent mode: neutrasearch-query --index INDEX.nsx --stdio");
                return Ok(());
            }
            x if x.starts_with('-') => bail!("unknown option {x}"),
            _ => i += 1,
        }
    }
    let path = index_path.unwrap_or_else(default_index_path);
    let (index, delta) = open_pair(&path)?;
    if stdio {
        return serve(
            &path,
            index,
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
        scope_roots,
        scope_case_sensitive: Some(cfg!(not(any(target_os = "windows", target_os = "macos")))),
    };
    let response = run(&index, delta.as_ref(), request)?;
    if json || json_paths {
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
    path: &std::path::Path,
    mut index: CompactIndex,
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
        let response = (|| {
            let base_replaced = CompactIndex::generation_on_disk(path)? != index.generation();
            let reopen = base_replaced
                || match &mut delta {
                    Some(delta) => delta.refresh().is_err(),
                    None => delta_path(path).is_file(),
                };
            if reopen {
                (index, delta) =
                    open_pair(path).context("reopen compact index after replacement")?;
            }
            run(&index, delta.as_ref(), request)
        })();
        match response {
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
    if request
        .scope_roots
        .iter()
        .any(|root| !std::path::Path::new(root).is_absolute())
    {
        bail!("scope_roots must contain only absolute paths");
    }
    let mut query = Query::parse(&request.query);
    query.limit = request.limit.unwrap_or(50).clamp(1, 1000);
    query.scope_roots = request.scope_roots;
    query.scope_case_sensitive = request
        .scope_case_sensitive
        .unwrap_or(cfg!(not(any(target_os = "windows", target_os = "macos"))));
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
fn open_pair(path: &std::path::Path) -> Result<(CompactIndex, Option<DeltaIndex>)> {
    CompactIndex::open_with_delta_snapshot(path)
        .with_context(|| format!("open compact index pair {}", path.display()))
}

fn delta_path(base: &std::path::Path) -> PathBuf {
    let mut path = base.to_path_buf();
    path.set_extension("delta");
    path
}

fn default_index_path() -> PathBuf {
    if let Some(path) =
        std::env::var_os("NEUTRASEARCH_INDEX").or_else(|| std::env::var_os("NEUTRA_INDEX"))
    {
        return path.into();
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(std::env::temp_dir)
            .join("Neutrasearch/index.nsx")
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .map(|home| home.join("Library/Application Support"))
            .unwrap_or_else(std::env::temp_dir)
            .join("Neutrasearch/index.nsx")
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .filter(|path| path.is_absolute())
                    .map(|home| home.join(".local/share"))
            })
            .unwrap_or_else(std::env::temp_dir)
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
        let (allowed_file, private_file, allowed_root) = if cfg!(target_os = "windows") {
            ("C:/src/needle.rs", "C:/private/needle-key.txt", "C:/src")
        } else {
            ("/src/needle.rs", "/private/needle-key.txt", "/src")
        };
        let records = vec![
            FileRecord {
                path: allowed_file.into(),
                size: 7,
                mtime: 1,
                mode: 0,
                kind: FileKind::File,
                fs: FsKind::Ext4,
                native_id: 1,
                native_parent: 2,
                source: 0,
            },
            FileRecord {
                path: private_file.into(),
                size: 9,
                mtime: 2,
                mode: 0,
                kind: FileKind::File,
                fs: FsKind::Ext4,
                native_id: 2,
                native_parent: 3,
                source: 0,
            },
        ];
        CompactIndex::build(&records, &path).unwrap();
        let index = CompactIndex::open(&path).unwrap();
        let mut output = Vec::new();
        let request = format!(
            "{{\"query\":\"needle\",\"limit\":5,\"scope_roots\":[{allowed_root:?}],\"scope_case_sensitive\":true}}\n"
        );
        serve(&path, index, None, request.as_bytes(), &mut output).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(value["paths"], serde_json::json!([allowed_file]));

        let index = CompactIndex::open(&path).unwrap();
        let error = match run(
            &index,
            None,
            Request {
                query: "needle".into(),
                limit: Some(5),
                metadata: Some(false),
                scope_roots: vec!["relative/path".into()],
                scope_case_sensitive: Some(true),
            },
        ) {
            Err(error) => error,
            Ok(_) => panic!("relative trusted scope must be rejected"),
        };
        assert!(error.to_string().contains("absolute paths"));
        std::fs::remove_file(path).unwrap();
    }
}
