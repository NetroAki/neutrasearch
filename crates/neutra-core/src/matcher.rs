//! Per-search text matching: term and regex scoring, sort comparison, and
//! highlight ranges. Split from `query` so the wire format stays separate
//! from the compiled matcher state.

use crate::query::{MatchFields, Query};
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
        self.score_path(&r.path)
    }

    /// Same scoring from the path alone, so scans can reject records before
    /// allocating them.
    pub fn score_path(&self, path: &str) -> Option<u32> {
        let query = &self.query;
        if query.terms.is_empty() && self.regex.is_none() {
            return Some(0);
        }
        let name = path.rfind('/').map_or(path, |i| &path[i + 1..]);
        let mut total: u64 = 0;
        if let Some(regex) = &self.regex {
            let name_hit = regex.find(name).map(|m| (m.start(), m.len()));
            let path_hit = match query.match_fields {
                MatchFields::Name => None,
                MatchFields::Path => regex.find(path).map(|m| (m.start(), m.len())),
                MatchFields::NameAndPath => {
                    if name_hit.is_some() {
                        None
                    } else {
                        regex.find(path).map(|m| (m.start(), m.len()))
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
                MatchFields::Path => (find(path), None),
                MatchFields::NameAndPath => (find(name), find(path)),
            };
            match hit {
                Some((pos, len)) => {
                    // Name match. Prefix matches and full-name matches score best.
                    let base: u64 = if pos == 0 { 1 << 20 } else { 1 << 12 };
                    let exact_bonus: u64 = if pos == 0 && len == name.len() {
                        1 << 24
                    } else {
                        0
                    };
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
    let exact_bonus: u64 = if pos == 0 && match_len == name_len {
        1 << 24
    } else {
        0
    };
    (base + exact_bonus + 256u64.saturating_sub(pos.min(255) as u64)).min(u32::MAX as u64) as u32
}

mod folding;
mod matching;
mod ordering;
use folding::strip_accents_str;
pub(crate) use matching::find_term;
pub(crate) use ordering::{cmp_name_ci, compare_records};

#[cfg(test)]
mod tests;
