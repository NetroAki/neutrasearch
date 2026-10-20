use super::fold_byte;

/// Join listing names while preserving drive and share roots under Computer.
pub fn join_child_path(dir: &str, name: &str) -> String {
    let bytes = name.as_bytes();
    if name.starts_with('/')
        || (bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1..3] == *b":/")
    {
        return name.to_owned();
    }
    let dir = dir.trim_end_matches('/');
    if dir.is_empty() {
        format!("/{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// Scan prefix for one directory: normalized form always ends in `/` so
/// the directory record itself sorts before the prefix and is skipped.
pub(super) fn dir_prefix(dir: &str) -> String {
    if dir == "/" {
        return "/".into();
    }
    let trimmed = dir.trim_end_matches('/');
    let mut prefix = String::with_capacity(trimmed.len() + 1);
    prefix.push_str(trimmed);
    prefix.push('/');
    prefix
}

/// Folded prefix match using the same byte fold as index ordering, so the
/// scan window agrees with the block sort order on every platform.
pub(super) fn starts_with_folded(path: &str, prefix: &str) -> bool {
    let (path, prefix) = (path.as_bytes(), prefix.as_bytes());
    path.len() >= prefix.len()
        && path[..prefix.len()]
            .iter()
            .zip(prefix.iter())
            .all(|(left, right)| fold_byte(*left) == fold_byte(*right))
}
