use anyhow::Result;
use std::io::{BufRead, Read, Write};

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

const MAX_REQUEST_BYTES: u64 = 4 << 20;

fn write_parse_error<W: Write>(writer: &mut W, message: &str) -> Result<()> {
    crate::view::write_json(
        writer,
        &serde_json::json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":message}}),
    )
}

pub(crate) fn serve<R: BufRead, W: Write>(
    mut index: Store,
    allowed_roots: &[std::path::PathBuf],
    reader: &mut R,
    writer: &mut W,
) -> Result<()> {
    let mut raw = Vec::new();
    loop {
        raw.clear();
        let read = (&mut *reader)
            .take(MAX_REQUEST_BYTES + 1)
            .read_until(b'\n', &mut raw)?;
        if read == 0 {
            break;
        }
        if raw.len() as u64 > MAX_REQUEST_BYTES && raw.last() != Some(&b'\n') {
            // Discard the rest of the oversized line without buffering it.
            loop {
                let mut discard = Vec::new();
                let more = (&mut *reader)
                    .take(MAX_REQUEST_BYTES)
                    .read_until(b'\n', &mut discard)?;
                if more == 0 || discard.last() == Some(&b'\n') {
                    break;
                }
            }
            write_parse_error(writer, "request exceeds the size limit")?;
            continue;
        }
        let line = String::from_utf8_lossy(&raw);
        if line.trim().is_empty() {
            continue;
        }
        let request: serde_json::Value = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(error) => {
                write_parse_error(writer, &format!("invalid JSON: {error}"))?;
                continue;
            }
        };
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
