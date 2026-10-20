pub(in crate::ui) fn is_drive_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'/'
}

pub(in crate::ui) fn normalize_path(path: &str) -> String {
    let replaced = path.replace('\\', "/");
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
    if is_drive_path(&replaced) && normalized.len() == 2 {
        normalized.push('/');
    }
    if normalized.is_empty() {
        "/".into()
    } else {
        normalized
    }
}

pub(in crate::ui) fn parent_path(path: &str) -> String {
    let normalized = normalize_path(path);
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

pub(in crate::ui) fn is_volume_root(path: &str) -> bool {
    if is_drive_path(path) {
        return path.len() == 3;
    }
    if let Some(unc) = path.strip_prefix("//") {
        return unc.split('/').filter(|part| !part.is_empty()).count() == 2;
    }
    false
}

pub(in crate::ui) fn path_name(path: &str) -> String {
    let normalized = normalize_path(path);
    if normalized == "/" {
        return "Computer".into();
    }
    if is_volume_root(&normalized) {
        return normalized.trim_end_matches('/').to_owned();
    }
    normalized
        .rsplit('/')
        .next()
        .unwrap_or(&normalized)
        .to_owned()
}

pub(in crate::ui) fn ancestor_paths(path: &str) -> Vec<String> {
    let normalized = normalize_path(path);
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
