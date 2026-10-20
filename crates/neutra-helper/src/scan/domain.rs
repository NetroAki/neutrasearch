#[path = "validation.rs"]
mod validation;
pub(crate) use validation::{validate_delta_changes, validate_query};

pub(crate) fn exclusion_prefixes(mountpoint: &std::path::Path) -> Vec<String> {
    let mut prefixes = vec![format!(
        "{}/.snapshots",
        mountpoint.to_string_lossy().trim_end_matches('/')
    )];
    if mountpoint == std::path::Path::new("/") {
        prefixes.push("/proc".into());
        prefixes.push("/sys".into());
    }
    prefixes
}

pub(crate) fn portable_path_in_root(path: &str, root: &std::path::Path) -> bool {
    let case_sensitive = cfg!(not(any(target_os = "windows", target_os = "macos")));
    let normalize = |value: &str| {
        let value = neutra_core::paths::portable_root_prefix(value);
        if case_sensitive {
            value
        } else {
            value.to_ascii_lowercase()
        }
    };
    neutra_core::paths::path_in_portable_root(&normalize(path), &normalize(&root.to_string_lossy()))
}

/// Hot-loop containment for a record path against a precomputed root prefix.
/// Case-insensitive platforms fold the record path per record (as before);
/// case-sensitive platforms are allocation-free.
pub(crate) fn record_in_root(path: &str, root: &str) -> bool {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        let path = path.to_ascii_lowercase();
        path == root
            || (root.ends_with('/') && path.starts_with(&root))
            || (path.len() > root.len()
                && !root.ends_with('/')
                && path.starts_with(&root)
                && path.as_bytes()[root.len()] == b'/')
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        neutra_core::paths::path_in_portable_root(path, root)
    }
}
pub(crate) fn path_has_component_prefix(path: &str, prefix: &str) -> bool {
    if path == prefix {
        return true;
    }
    path.len() > prefix.len() && path.starts_with(prefix) && path.as_bytes()[prefix.len()] == b'/'
}

pub(crate) fn same_portable_path(left: &std::path::Path, right: &std::path::Path) -> bool {
    let case_sensitive = cfg!(not(any(target_os = "windows", target_os = "macos")));
    let normalize = |path: &std::path::Path| {
        let value = path.to_string_lossy().replace('\\', "/");
        if case_sensitive {
            value
        } else {
            value.to_ascii_lowercase()
        }
    };
    normalize(left).trim_end_matches('/') == normalize(right).trim_end_matches('/')
}
pub(crate) fn safe_absolute_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    let windows_absolute = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\')
        || path.starts_with("\\\\");
    !path.contains('\0')
        && (std::path::Path::new(path).is_absolute() || windows_absolute)
        && !path
            .split(['/', '\\'])
            .any(|component| matches!(component, "." | ".."))
}
