//! Per-search text matching: term and regex scoring, sort comparison, and
//! highlight ranges. Split from `query` so the wire format stays separate
//! from the compiled matcher state.

use crate::query::{MatchFields, Query, SortKey};
use crate::types::FileRecord;

/// Per-search compiled text matcher. Owns the query so callers can cache it
/// across frames; only this holds the compiled regex.
pub struct QueryMatcher {
    query: Query,
    /// `to_lowercase()` of each term, aligned by index; drives the
    /// Unicode-aware case-insensitive search.
    folded_terms: Vec<String>,
    regex: Option<regex::Regex>,
}

impl QueryMatcher {
    pub(crate) fn new(query: Query) -> std::io::Result<Self> {
        // Fold needles once per search. The finder must never rely on the
        // caller pre-lowercasing: uppercase non-ASCII terms ("CAFÉ")
        // previously failed against lowercase haystacks because only the
        // haystack was folded.
        // haystack was folded. With `fold_accents`, accents strip here so
        // the per-record hot loop compares base letters directly.
        let folded_terms = query
            .terms
            .iter()
            .map(|term| {
                if query.fold_accents {
                    if query.case_sensitive {
                        strip_accents_str(term)
                    } else {
                        strip_accents_str(&term.to_lowercase())
                    }
                } else {
                    term.to_lowercase()
                }
            })
            .collect::<Vec<_>>();
        let regex = match &query.regex {
            Some(pattern) => Some(
                regex::RegexBuilder::new(pattern)
                    .case_insensitive(!query.case_sensitive)
                    .build()
                    .map_err(|error| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!("invalid query regex: {error}"),
                        )
                    })?,
            ),
            None => None,
        };
        Ok(Self {
            query,
            folded_terms,
            regex,
        })
    }

    /// The compiled query, for reuse.
    pub fn query(&self) -> &Query {
        &self.query
    }

    /// Byte ranges in the record's file name that should be highlighted,
    /// mirroring `score`'s term and regex matching. Capped so pathological
    /// patterns cannot flood the UI.
    pub fn name_match_ranges(&self, r: &FileRecord) -> Vec<std::ops::Range<usize>> {
        const MAX_RANGES: usize = 8;
        let name = r.name();
        let mut out = Vec::new();
        if let Some(regex) = &self.regex {
            if self.query.match_fields == MatchFields::Path {
                return out;
            }
            for mat in regex.find_iter(name) {
                if out.len() >= MAX_RANGES {
                    break;
                }
                out.push(mat.start()..mat.end());
            }
            return out;
        }
        for (index, term) in self.query.terms.iter().enumerate() {
            if out.len() >= MAX_RANGES {
                break;
            }
            let query = &self.query;
            let hit = find_term(
                name,
                term,
                &self.folded_terms[index],
                query.case_sensitive,
                query.whole_word,
                query.fold_accents,
            );
            if let Some((start, len)) = hit {
                // Folding can shift offsets for exotic scripts; skip
                // ranges that would render outside the name.
                if start + len <= name.len() {
                    let range = start..start + len;
                    if !out.contains(&range) {
                        out.push(range);
                    }
                }
            }
        }
        out
    }

    /// Relevance score for term/regex matching; `None` = no match.
    pub fn score(&self, r: &FileRecord) -> Option<u32> {
        let query = &self.query;
        if query.terms.is_empty() && self.regex.is_none() {
            return Some(0);
        }
        let name = r.name();
        let mut total: u64 = 0;
        if let Some(regex) = &self.regex {
            let name_hit = regex.find(name).map(|m| (m.start(), m.len()));
            let path_hit = match query.match_fields {
                MatchFields::Name => None,
                MatchFields::Path => regex.find(&r.path).map(|m| (m.start(), m.len())),
                MatchFields::NameAndPath => {
                    if name_hit.is_some() {
                        None
                    } else {
                        regex.find(&r.path).map(|m| (m.start(), m.len()))
                    }
                }
            };
            return Some(match (name_hit, path_hit) {
                (Some((pos, len)), _) => name_score(pos, len, name.len()),
                (None, Some(_)) => 1 << 6,
                (None, None) => return None,
            });
        }
        for (index, term) in query.terms.iter().enumerate() {
            let folded = &self.folded_terms[index];
            let find = |haystack: &str| {
                find_term(
                    haystack,
                    term,
                    folded,
                    query.case_sensitive,
                    query.whole_word,
                    query.fold_accents,
                )
            };
            let (hit, fallback) = match query.match_fields {
                MatchFields::Name => (find(name), None),
                MatchFields::Path => (find(&r.path), None),
                MatchFields::NameAndPath => (find(name), find(&r.path)),
            };
            match hit {
                Some((pos, len)) => {
                    // Name match. Prefix matches and full-name matches score best.
                    let base: u64 = if pos == 0 { 1 << 20 } else { 1 << 12 };
                    let exact_bonus: u64 =
                        if pos == 0 && len == name.len() { 1 << 24 } else { 0 };
                    total += base + exact_bonus + 256u64.saturating_sub(pos.min(255) as u64);
                }
                None => {
                    fallback?;
                    total += 1 << 6;
                }
            }
        }
        Some(total.min(u32::MAX as u64) as u32)
    }
}

fn name_score(pos: usize, match_len: usize, name_len: usize) -> u32 {
    let base: u64 = if pos == 0 { 1 << 20 } else { 1 << 12 };
    let exact_bonus: u64 = if pos == 0 && match_len == name_len { 1 << 24 } else { 0 };
    (base + exact_bonus + 256u64.saturating_sub(pos.min(255) as u64)).min(u32::MAX as u64) as u32
}

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

/// Normalize one haystack character the same way the needle was folded:
/// strip accents when asked, then lowercase unless case-sensitive.
#[inline]
fn norm_char(c: char, case_sensitive: bool, fold_accents: bool) -> char {
    let mut c = if fold_accents { strip_accent(c) } else { c };
    if !case_sensitive {
        c = c.to_lowercase().next().unwrap_or(c);
    }
    c
}

/// Strip common Latin diacritics to their base letter, preserving case so
/// Strip common Latin diacritics to their base letter, preserving case so
/// case-sensitive search still tells `E-acute` from `e`. Unlisted scripts
/// pass through untouched (same semantics as before for those).
// ponytail: Latin-only accent table, upgrade to NFKD folding if non-Latin
// accent-insensitive search is ever requested.
#[inline]
fn strip_accent(c: char) -> char {
    match c {
        '\u{e0}' | '\u{e1}' | '\u{e2}' | '\u{e3}' | '\u{e4}' | '\u{e5}' | '\u{101}' | '\u{103}' | '\u{105}' => 'a',
        '\u{c0}' | '\u{c1}' | '\u{c2}' | '\u{c3}' | '\u{c4}' | '\u{c5}' | '\u{100}' | '\u{102}' | '\u{104}' => 'A',
        '\u{e8}' | '\u{e9}' | '\u{ea}' | '\u{eb}' | '\u{113}' | '\u{115}' | '\u{117}' | '\u{119}' | '\u{11b}' => 'e',
        '\u{c8}' | '\u{c9}' | '\u{ca}' | '\u{cb}' | '\u{112}' | '\u{114}' | '\u{116}' | '\u{118}' | '\u{11a}' => 'E',
        '\u{ec}' | '\u{ed}' | '\u{ee}' | '\u{ef}' | '\u{12b}' | '\u{12d}' | '\u{12f}' | '\u{131}' => 'i',
        '\u{cc}' | '\u{cd}' | '\u{ce}' | '\u{cf}' | '\u{12a}' | '\u{12c}' | '\u{12e}' | '\u{130}' => 'I',
        '\u{f2}' | '\u{f3}' | '\u{f4}' | '\u{f5}' | '\u{f6}' | '\u{f8}' | '\u{14d}' | '\u{14f}' | '\u{151}' => 'o',
        '\u{d2}' | '\u{d3}' | '\u{d4}' | '\u{d5}' | '\u{d6}' | '\u{d8}' | '\u{14c}' | '\u{14e}' | '\u{150}' => 'O',
        '\u{f9}' | '\u{fa}' | '\u{fb}' | '\u{fc}' | '\u{16b}' | '\u{16d}' | '\u{16f}' | '\u{171}' | '\u{173}' => 'u',
        '\u{d9}' | '\u{da}' | '\u{db}' | '\u{dc}' | '\u{16a}' | '\u{16c}' | '\u{16e}' | '\u{170}' | '\u{172}' => 'U',
        '\u{fd}' | '\u{ff}' | '\u{177}' => 'y',
        '\u{dd}' | '\u{178}' | '\u{176}' => 'Y',
        '\u{e7}' | '\u{107}' | '\u{109}' | '\u{10d}' => 'c',
        '\u{c7}' | '\u{106}' | '\u{108}' | '\u{10c}' => 'C',
        '\u{f1}' | '\u{144}' | '\u{146}' | '\u{148}' => 'n',
        '\u{d1}' | '\u{143}' | '\u{145}' | '\u{147}' => 'N',
        '\u{15b}' | '\u{15d}' | '\u{15f}' | '\u{161}' => 's',
        '\u{15a}' | '\u{15c}' | '\u{15e}' | '\u{160}' => 'S',
        '\u{17a}' | '\u{17c}' | '\u{17e}' => 'z',
        '\u{179}' | '\u{17b}' | '\u{17d}' => 'Z',
        '\u{11d}' | '\u{11f}' | '\u{121}' | '\u{123}' => 'g',
        '\u{11c}' | '\u{11e}' | '\u{120}' | '\u{122}' => 'G',
        '\u{13a}' | '\u{13c}' | '\u{13e}' | '\u{140}' | '\u{142}' => 'l',
        '\u{139}' | '\u{13b}' | '\u{13d}' | '\u{13f}' | '\u{141}' => 'L',
        '\u{155}' | '\u{157}' | '\u{159}' => 'r',
        '\u{154}' | '\u{156}' | '\u{158}' => 'R',
        _ => c,
    }
}

/// Strip accents from a whole string (needle pre-folding).
fn strip_accents_str(s: &str) -> String {
    s.chars().map(strip_accent).collect()
}

/// ASCII case-insensitive name comparison without per-comparison allocations.
/// Used by sort comparators, which run O(n log n) times per query.
#[inline]
pub(crate) fn cmp_name_ci(left: &str, right: &str) -> std::cmp::Ordering {
    left.bytes()
        .map(|b| b.to_ascii_lowercase())
        .cmp(right.bytes().map(|b| b.to_ascii_lowercase()))
}

/// Timestamps past 2100 come from corrupt archives and would pin themselves
/// to the top of every newest-first listing, so they sort as unknown.
const LATEST_PLAUSIBLE_MTIME: i64 = 4_102_444_800;

fn sort_mtime(record: &FileRecord) -> i64 {
    if record.mtime > LATEST_PLAUSIBLE_MTIME { 0 } else { record.mtime }
}

/// The single sort comparator shared by the in-memory and compact engines.
/// `a`/`b` carry the relevance score plus the record; ties fall back to path
/// so ordering is deterministic.
pub(crate) fn compare_records(
    sort: SortKey,
    a: &(u32, &FileRecord),
    b: &(u32, &FileRecord),
) -> std::cmp::Ordering {
    match sort {
        SortKey::Relevance => b
            .0
            .cmp(&a.0)
            .then(b.1.mtime.cmp(&a.1.mtime))
            .then(a.1.path.cmp(&b.1.path)),
        SortKey::NameAsc => cmp_name_ci(a.1.name(), b.1.name()).then(a.1.path.cmp(&b.1.path)),
        SortKey::NameDesc => cmp_name_ci(b.1.name(), a.1.name()).then(b.1.path.cmp(&a.1.path)),
        SortKey::PathAsc => a.1.path.cmp(&b.1.path),
        SortKey::PathDesc => b.1.path.cmp(&a.1.path),
        SortKey::SizeDesc => b.1.size.cmp(&a.1.size).then(a.1.path.cmp(&b.1.path)),
        SortKey::SizeAsc => a.1.size.cmp(&b.1.size).then(a.1.path.cmp(&b.1.path)),
        SortKey::MtimeDesc => sort_mtime(b.1).cmp(&sort_mtime(a.1)).then(a.1.path.cmp(&b.1.path)),
        SortKey::MtimeAsc => sort_mtime(a.1).cmp(&sort_mtime(b.1)).then(a.1.path.cmp(&b.1.path)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_needles_fold_without_pre_lowercasing() {
        // Regression: uppercase non-ASCII needles previously never matched
        // because only the haystack was folded.
        // Matches return (byte offset, byte length); non-ASCII letters span
        // multiple bytes in UTF-8.
        assert_eq!(find_term("café.md", "CAFÉ", "café", false, false, false), Some((0, 5)));
        assert_eq!(find_term("ärger.txt", "ÄRGER", "ärger", false, false, false), Some((0, 6)));
        assert_eq!(find_term("文件.txt", "文件", "文件", false, false, false), Some((0, 6)));
        assert_eq!(find_term("report.pdf", "MISSING", "missing", false, false, false), None);
        // `to_lowercase` is not full case folding: ß does not match "SS".
        // Same semantics as before this refactor; documented, not accidental.
        assert_eq!(find_term("Straße.txt", "STRASSE", "strasse", false, false, false), None);
    }

    #[test]
    fn ascii_paths_stay_on_the_fast_byte_path() {
        assert_eq!(find_term("/a/Report.PDF", "report", "report", false, false, false), Some((3, 6)));
    }

    #[test]
    fn whole_word_rejects_substrings() {
        let folded = "call".to_string();
        assert_eq!(
            find_term("the calling.wav", "call", &folded, false, true, false),
            None
        );
        assert_eq!(
            find_term("the call.wav", "call", &folded, false, true, false),
            Some((4, 4))
        );
        assert_eq!(
            find_term("recall.wav", "call", &folded, false, false, false),
            Some((2, 4))
        );
    }

    #[test]
    fn accent_folding_is_opt_in() {
        let strict = "cafe".to_string();
        assert_eq!(
            find_term("caf\u{e9}.wav", "cafe", &strict, false, false, false),
            None
        );
        let folded = strip_accents_str("cafe");
        assert_eq!(
            find_term("caf\u{e9}.wav", "cafe", &folded, false, false, true),
            // `\u{e9}` is two bytes in UTF-8, so the match spans 5 bytes.
            Some((0, 5))
        );
    }
}
