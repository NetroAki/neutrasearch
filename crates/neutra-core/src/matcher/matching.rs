use super::folding::norm_char;

/// Case-insensitive substring search. ASCII takes a fast allocation-free
/// byte path; other scripts compare through Unicode case folding without
/// materializing a lowercased haystack (which previously cost one `String`
/// per record per term and silently missed when the *needle* itself was not
/// folded). `needle_folded` must be `needle.to_lowercase()`.
#[inline]
/// Term search with whole-word and accent-folding options. Returns the byte
/// offset and matched byte length. ASCII stays on the fast byte path
/// (accent folding is a no-op there); other scripts compare character by
/// character so `fold_accents` can strip diacritics on both sides.
/// `needle_folded` must be the needle run through the same
/// lowercase/strip pipeline as the matcher constructor.
pub(crate) fn find_term(
    haystack: &str,
    needle: &str,
    needle_folded: &str,
    case_sensitive: bool,
    whole_word: bool,
    fold_accents: bool,
) -> Option<(usize, usize)> {
    if needle.is_empty() {
        return Some((0, 0));
    }
    if haystack.is_ascii() && needle.is_ascii() && !fold_accents {
        let h = haystack.as_bytes();
        let n = needle.as_bytes();
        if n.len() > h.len() {
            return None;
        }
        for start in 0..=h.len() - n.len() {
            let same = if case_sensitive {
                &h[start..start + n.len()] == n
            } else {
                h[start..start + n.len()].eq_ignore_ascii_case(n)
            };
            if same && (!whole_word || ascii_word_bounds(h, start, n.len())) {
                return Some((start, n.len()));
            }
        }
        return None;
    }
    // General path: normalize both sides per character. Needle length in
    // characters bounds the scan; byte length is measured from the haystack.
    let normed_needle: Vec<char> = if case_sensitive && !fold_accents {
        needle.chars().collect()
    } else {
        needle_folded.chars().collect()
    };
    if normed_needle.is_empty() {
        return Some((0, 0));
    }
    for (start, _) in haystack.char_indices() {
        let mut rest = haystack[start..].chars();
        let mut end = start;
        let mut matched = true;
        for expected in &normed_needle {
            match rest.next() {
                Some(actual) => {
                    end += actual.len_utf8();
                    if norm_char(actual, case_sensitive, fold_accents) != *expected {
                        matched = false;
                        break;
                    }
                }
                None => {
                    matched = false;
                    break;
                }
            }
        }
        if matched {
            if whole_word {
                let prev = haystack[..start].chars().next_back();
                let next = haystack[end..].chars().next();
                if prev.is_some_and(is_word_char) || next.is_some_and(is_word_char) {
                    continue;
                }
            }
            return Some((start, end - start));
        }
    }
    None
}

#[inline]
fn ascii_word_bounds(haystack: &[u8], start: usize, len: usize) -> bool {
    let prev_ok = start == 0 || !is_word_byte(haystack[start - 1]);
    let next_ok = start + len >= haystack.len() || !is_word_byte(haystack[start + len]);
    prev_ok && next_ok
}

#[inline]
fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

#[inline]
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}
