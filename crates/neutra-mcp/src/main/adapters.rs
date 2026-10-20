use anyhow::Result;
use std::io::{BufRead, Write};

use crate::{domain, policy, state, store::Store};

pub(crate) fn run() -> Result<()> {
    let index_path = state::configured_index_from(std::env::var_os("NEUTRASEARCH_INDEX"))?;
    let allowed_roots =
        policy::allowed_roots_from(std::env::var_os("NEUTRASEARCH_MCP_ALLOWED_ROOTS"))?;
    serve(
        Store::open(index_path)?,
        &allowed_roots,
        &mut std::io::stdin().lock(),
        &mut std::io::stdout().lock(),
    )
}

pub(crate) fn serve<R: BufRead, W: Write>(
    mut index: Store,
    allowed_roots: &[std::path::PathBuf],
    reader: &mut R,
    writer: &mut W,
) -> Result<()> {
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let request: serde_json::Value = serde_json::from_str(&line)?;
        let Some(id) = request.get("id").cloned() else {
            continue;
        };
        let method = request
            .get("method")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let Some(result) =
            domain::dispatch(&mut index, allowed_roots, &request, method, &id, writer)?
        else {
            continue;
        };
        crate::view::write_json(
            writer,
            &serde_json::json!({"jsonrpc":"2.0","id":id,"result":result}),
        )?;
    }
    Ok(())
}
