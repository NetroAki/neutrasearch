use super::*;

pub(crate) fn entry_key(source: u32, path: &str) -> Key {
    PathKey {
        source,
        canonical: canonical_path(path),
    }
}

pub(super) fn compare_key(
    left_source: u32,
    left_path: &str,
    right_source: u32,
    right_path: &str,
) -> Ordering {
    left_source
        .cmp(&right_source)
        .then_with(|| compare_paths(left_path, right_path))
}

pub(crate) fn compare_paths(left: &str, right: &str) -> Ordering {
    let left = left.as_bytes();
    let right = right.as_bytes();
    let fold = |byte: u8| {
        let byte = if byte == b'\\' { b'/' } else { byte };
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        {
            byte.to_ascii_lowercase()
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            byte
        }
    };
    for (left, right) in left.iter().zip(right) {
        match fold(*left).cmp(&fold(*right)) {
            Ordering::Equal => {}
            other => return other,
        }
    }
    left.len().cmp(&right.len())
}

pub(crate) fn canonical_path(path: &str) -> String {
    let path = path.replace('\\', "/");
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        path.to_lowercase()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        path
    }
}

pub(crate) fn normalize_path(path: &str) -> io::Result<String> {
    let replaced = path.replace('\\', "/");
    let absolute = replaced.starts_with('/')
        || (replaced.len() >= 3
            && replaced.as_bytes()[0].is_ascii_alphabetic()
            && replaced.as_bytes()[1] == b':'
            && replaced.as_bytes()[2] == b'/');
    if !absolute
        || replaced
            .split('/')
            .any(|component| matches!(component, "." | ".."))
    {
        return Err(invalid(format!("unsafe or relative summary path {path}")));
    }
    let unc = replaced.starts_with("//");
    let mut normalized = replaced
        .split('/')
        .filter(|component| !component.is_empty())
        .collect::<Vec<_>>()
        .join("/");
    if unc {
        normalized.insert_str(0, "//");
    } else if replaced.starts_with('/') {
        normalized.insert(0, '/');
    }
    if replaced.len() >= 3
        && replaced.as_bytes()[0].is_ascii_alphabetic()
        && replaced.as_bytes()[1] == b':'
        && normalized.len() == 2
    {
        normalized.push('/');
    }
    if normalized.is_empty() {
        normalized.push('/');
    }
    Ok(normalized)
}

pub(crate) fn parent_path(path: &str) -> String {
    let normalized = normalize_path(path).unwrap_or_else(|_| path.to_owned());
    if normalized == "/" || is_volume_root(&normalized) {
        return "/".into();
    }
    normalized.rsplit_once('/').map_or_else(
        || "/".into(),
        |(parent, _)| {
            if parent.is_empty() {
                "/".into()
            } else if parent.len() == 2 && parent.ends_with(':') {
                format!("{parent}/")
            } else {
                parent.into()
            }
        },
    )
}

pub(crate) fn ancestor_paths(path: &str) -> Vec<String> {
    let normalized = normalize_path(path).unwrap_or_else(|_| path.to_owned());
    let mut out = vec!["/".to_owned()];
    if is_drive_path(&normalized) {
        let root = normalized[..3].to_owned();
        out.push(root.clone());
        let mut current = root;
        for component in normalized[3..]
            .split('/')
            .filter(|component| !component.is_empty())
        {
            current.push_str(component);
            out.push(current.clone());
            current.push('/');
        }
    } else if let Some(unc) = normalized.strip_prefix("//") {
        let mut components = unc.split('/').filter(|component| !component.is_empty());
        if let (Some(server), Some(share)) = (components.next(), components.next()) {
            let mut current = format!("//{server}/{share}");
            out.push(current.clone());
            for component in components {
                current.push('/');
                current.push_str(component);
                out.push(current.clone());
            }
        }
    } else {
        let mut current = String::new();
        for component in normalized
            .split('/')
            .filter(|component| !component.is_empty())
        {
            current.push('/');
            current.push_str(component);
            out.push(current.clone());
        }
    }
    out.dedup();
    out
}

pub(super) fn is_drive_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'/'
}

pub(super) fn is_volume_root(path: &str) -> bool {
    if is_drive_path(path) {
        return path.len() == 3;
    }
    if let Some(unc) = path.strip_prefix("//") {
        return unc.split('/').filter(|part| !part.is_empty()).count() == 2;
    }
    false
}

pub(crate) fn directory_summary_path(index_path: &Path) -> PathBuf {
    let mut value = index_path.as_os_str().to_os_string();
    value.push(".dirs");
    value.into()
}
