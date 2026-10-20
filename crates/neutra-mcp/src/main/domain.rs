use anyhow::Result;
use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;

use crate::{store::Store, tools::call_tool, view};

pub(crate) fn dispatch(
    index: &mut Store,
    allowed_roots: &[PathBuf],
    request: &Value,
    method: &str,
    id: &Value,
    writer: &mut impl Write,
) -> Result<Option<Value>> {
    let result = match method {
        "initialize" => {
            json!({"protocolVersion":"2025-03-26","capabilities":{"tools":{}},"serverInfo":{"name":"neutrasearch","version":env!("CARGO_PKG_VERSION")}})
        }
        "tools/list" => crate::tools::tools_list(),
        "tools/call" => call_tool(
            index,
            allowed_roots,
            request
                .pointer("/params/name")
                .and_then(Value::as_str)
                .unwrap_or(""),
            request
                .pointer("/params/arguments")
                .cloned()
                .unwrap_or_else(|| json!({})),
        ),
        "ping" => json!({}),
        _ => {
            view::write_json(
                writer,
                &json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":format!("unknown method {method}")}}),
            )?;
            return Ok(None);
        }
    };
    Ok(Some(result))
}

#[cfg(test)]
#[path = "domain/tests.rs"]
mod tests;
