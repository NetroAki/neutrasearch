//! Persistent NDJSON service and request/response shaping for the query CLI.
//! Split from `main` so argument parsing, transport, and response shaping
//! stay separately owned.

use anyhow::{bail, Context, Result};
use neutra_core::{CompactIndex, DeltaIndex, Query, SearchHit, SearchStats};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, Write};
use std::path::PathBuf;

#[derive(Deserialize)]
pub struct Request {
    pub query: String,
    pub limit: Option<usize>,
    pub metadata: Option<bool>,
    #[serde(default)]
    pub scope_roots: Vec<String>,
    #[serde(default)]
    pub scope_case_sensitive: Option<bool>,
}
#[derive(Serialize)]
pub struct Response {
    pub paths: Vec<String>,
    pub matched: u64,
    pub returned: usize,
    pub search_us: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub records: Option<Vec<Record>>,
}
#[derive(Serialize)]
pub struct Record {
    path: String,
    kind: String,
    size: u64,
    mtime: i64,
    fs: String,
}

pub fn serve(
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
pub fn run(index: &CompactIndex, delta: Option<&DeltaIndex>, request: Request) -> Result<Response> {
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
pub fn open_pair(path: &std::path::Path) -> Result<(CompactIndex, Option<DeltaIndex>)> {
    CompactIndex::open_with_delta_snapshot_fast(path)
        .with_context(|| format!("open compact index pair {}", path.display()))
}

pub(crate) fn delta_path(base: &std::path::Path) -> PathBuf {
    let mut path = base.to_path_buf();
    path.set_extension("delta");
    path
}
