use super::*;

/// One canonical path-safety predicate for the crate: absolute (portable or
/// Windows), no NUL, and no `.`/`..` components.
#[inline]
pub(crate) fn safe_absolute_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    let windows_absolute = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\')
        || path.starts_with("\\\\");
    let portable_absolute = path.starts_with('/') || windows_absolute;
    !path.contains('\0')
        && portable_absolute
        && !path
            .split(if is_windows_path(path) {
                &['/', '\\'][..]
            } else {
                &['/'][..]
            })
            .any(|component| matches!(component, "." | ".."))
}

#[inline]
pub(super) fn path_is_under(path: &str, root: &str) -> bool {
    component_path_is_under(
        path,
        root,
        is_windows_path(root) || is_windows_path(path),
        true,
    )
}

#[inline]
pub(super) fn path_is_under_ci(path: &str, lower_root: &str) -> bool {
    component_path_is_under(
        path,
        lower_root,
        is_windows_path(lower_root) || is_windows_path(path),
        false,
    )
}

fn is_windows_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    cfg!(target_os = "windows")
        || (bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'/' | b'\\'))
        || path.starts_with("\\\\")
}

fn component_path_is_under(path: &str, root: &str, windows: bool, case_sensitive: bool) -> bool {
    use std::borrow::Cow;

    let path = if case_sensitive || path.is_ascii() {
        Cow::Borrowed(path)
    } else {
        Cow::Owned(path.to_lowercase())
    };
    let root = if case_sensitive || root.is_ascii() {
        Cow::Borrowed(root)
    } else {
        Cow::Owned(root.to_lowercase())
    };
    let Some(prefix) = path.as_bytes().get(..root.len()) else {
        return false;
    };
    let same_prefix = prefix.iter().zip(root.as_bytes()).all(|(left, right)| {
        if windows
            && left != right
            && matches!(*left, b'/' | b'\\')
            && matches!(*right, b'/' | b'\\')
        {
            true
        } else if case_sensitive {
            left == right
        } else {
            left.eq_ignore_ascii_case(right)
        }
    });
    if !same_prefix {
        return false;
    }
    path.len() == root.len()
        || is_path_separator(root.as_bytes().last().copied(), windows)
        || is_path_separator(path.as_bytes().get(root.len()).copied(), windows)
}

#[inline]
fn is_path_separator(byte: Option<u8>, windows: bool) -> bool {
    matches!(byte, Some(b'/')) || (windows && matches!(byte, Some(b'\\')))
}

/// Split input into tokens, respecting double quotes.
pub(super) fn tokenize(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    for c in input.chars() {
        match c {
            '"' => in_quotes = !in_quotes,
            c if c.is_whitespace() && !in_quotes => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

pub(super) fn parse_size(spec: &str, q: &mut Query) {
    if let Some((a, b)) = spec.split_once("..") {
        q.min_size = parse_size_num(a);
        q.max_size = parse_size_num(b);
    } else if let Some(rest) = spec.strip_prefix('>') {
        q.min_size = parse_size_num(rest);
    } else if let Some(rest) = spec.strip_prefix('<') {
        q.max_size = parse_size_num(rest);
    } else {
        // bare number = minimum
        q.min_size = parse_size_num(spec);
    }
}

pub(super) fn parse_size_num(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (digits, mult) = match s.chars().last()? {
        'k' | 'K' => (&s[..s.len() - 1], 1u64 << 10),
        'm' | 'M' => (&s[..s.len() - 1], 1u64 << 20),
        'g' | 'G' => (&s[..s.len() - 1], 1u64 << 30),
        't' | 'T' => (&s[..s.len() - 1], 1u64 << 40),
        _ => (s, 1),
    };
    digits.parse::<u64>().ok().map(|n| n * mult)
}
