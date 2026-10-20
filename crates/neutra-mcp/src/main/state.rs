use anyhow::{bail, Result};
use std::ffi::OsString;
use std::path::PathBuf;

pub(crate) fn configured_index_from(configured: Option<OsString>) -> Result<PathBuf> {
    if let Some(path) = configured {
        if path.is_empty() {
            bail!("configured MCP index path must not be empty");
        }
        return Ok(PathBuf::from(path));
    }
    Ok(neutra_core::paths::resolve_index_path(None))
}
