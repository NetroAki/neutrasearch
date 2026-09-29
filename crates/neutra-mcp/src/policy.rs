//! Search-scope policy for the MCP server: allowed roots from the
//! environment and the portable path-containment checks that enforce them.

use anyhow::{bail, Context, Result};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub fn allowed_roots_from(value: Option<OsString>) -> Result<Vec<PathBuf>> {
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

pub fn path_is_allowed(path: &Path, allowed_roots: &[PathBuf]) -> bool {
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

pub(crate) fn portable_path_text(path: &str) -> String {
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

pub(crate) fn portable_path_is_under(path: &str, root: &str, case_sensitive: bool) -> bool {
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
