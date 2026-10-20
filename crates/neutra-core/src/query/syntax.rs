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
            .split(['/', '\\'])
            .any(|component| matches!(component, "." | ".."))
}

#[inline]
pub(super) fn path_is_under(path: &str, root: &str) -> bool {
    path.strip_prefix(root).is_some_and(|rest| {
        rest.is_empty()
            || root.ends_with('/')
            || root.ends_with('\\')
            || rest.starts_with('/')
            || rest.starts_with('\\')
    })
}

#[inline]
pub(super) fn path_is_under_ci(path: &str, lower_root: &str) -> bool {
    let (path, root) = if path.is_ascii() && lower_root.is_ascii() {
        (
            std::borrow::Cow::Borrowed(path),
            std::borrow::Cow::Borrowed(lower_root),
        )
    } else {
        (
            std::borrow::Cow::Owned(path.to_lowercase()),
            std::borrow::Cow::Owned(lower_root.to_lowercase()),
        )
    };
    let Some(prefix) = path.as_bytes().get(..root.len()) else {
        return false;
    };
    let same_prefix = prefix.iter().zip(root.as_bytes()).all(|(left, right)| {
        left.eq_ignore_ascii_case(right)
            || (matches!(left, b'/' | b'\\') && matches!(right, b'/' | b'\\'))
    });
    if !same_prefix {
        return false;
    }
    path.len() == root.len()
        || root.ends_with('/')
        || root.ends_with('\\')
        || path
            .as_bytes()
            .get(root.len())
            .is_some_and(|next| matches!(next, b'/' | b'\\'))
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
