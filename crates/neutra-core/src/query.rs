//! Query parsing and record matching.
//!
//! Syntax (Everything-flavoured, deliberately small):
//!   plain words        case-insensitive substrings, all must match the name or path
//!   ext:rs,toml        extension filter (comma = OR)
//!   kind:file|dir|link type filter
//!   fs:btrfs|ext4|ntfs|zfs
//!   size:>100M  size:<4k  size:1M..2M
//!   under:/some/dir    path prefix filter
//!   "exact phrase"     quoted substring with spaces
//!
//! Sorting: relevance by default (name-prefix > name > path, then mtime desc).

use crate::mounts::FsKind;
use crate::types::{FileKind, FileRecord};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortKey {
    Relevance,
    NameAsc,
    SizeDesc,
    MtimeDesc,
    PathAsc,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Query {
    /// Lowercased substrings that must ALL match (name or path).
    pub terms: Vec<String>,
    pub exts: Vec<String>,
    pub kinds: Vec<FileKind>,
    pub fss: Vec<FsKind>,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    /// Lowercased path prefix.
    pub under: Option<String>,
    pub sort: SortKey,
    /// Hard cap on returned hits; 0 = unlimited.
    pub limit: usize,
}

impl Default for Query {
    fn default() -> Self {
        Query {
            terms: Vec::new(),
            exts: Vec::new(),
            kinds: Vec::new(),
            fss: Vec::new(),
            min_size: None,
            max_size: None,
            under: None,
            sort: SortKey::Relevance,
            limit: 1000,
        }
    }
}

impl Query {
    pub fn parse(input: &str) -> Query {
        let mut q = Query::default();
        for tok in tokenize(input) {
            if let Some(rest) = tok.strip_prefix("ext:") {
                q.exts.extend(
                    rest.split(',')
                        .filter(|s| !s.is_empty())
                        .map(|s| s.trim_start_matches('.').to_lowercase()),
                );
            } else if let Some(rest) = tok.strip_prefix("kind:") {
                for k in rest.split(',') {
                    let k = match k {
                        "file" | "f" => Some(FileKind::File),
                        "dir" | "d" | "folder" => Some(FileKind::Dir),
                        "link" | "symlink" | "l" => Some(FileKind::Symlink),
                        _ => None,
                    };
                    if let Some(k) = k {
                        q.kinds.push(k);
                    }
                }
            } else if let Some(rest) = tok.strip_prefix("fs:") {
                q.fss.extend(
                    rest.split(',')
                        .filter(|s| !s.is_empty())
                        .map(FsKind::from_fstype),
                );
            } else if let Some(rest) = tok.strip_prefix("size:") {
                parse_size(rest, &mut q);
            } else if let Some(rest) = tok.strip_prefix("under:") {
                q.under = Some(rest.to_lowercase());
            } else if !tok.is_empty() {
                q.terms.push(tok.to_lowercase());
            }
        }
        q
    }

    /// Cheap filter phase (no term matching). Run before the term phase.
    #[inline]
    pub fn passes_filters(&self, r: &FileRecord) -> bool {
        if !self.kinds.is_empty() && !self.kinds.contains(&r.kind) {
            return false;
        }
        if !self.fss.is_empty() && !self.fss.contains(&r.fs) {
            return false;
        }
        if !self.exts.is_empty() {
            let ext = r.extension();
            // extension() borrows from path; compare case-insensitively
            let mut ok = false;
            for want in &self.exts {
                if ext.len() == want.len() && ext.eq_ignore_ascii_case(want) {
                    ok = true;
                    break;
                }
            }
            if !ok {
                return false;
            }
        }
        if let Some(min) = self.min_size {
            if r.size < min {
                return false;
            }
        }
        if let Some(max) = self.max_size {
            if r.size > max {
                return false;
            }
        }
        if let Some(under) = &self.under {
            if !starts_with_ci(&r.path, under) {
                return false;
            }
        }
        true
    }

    /// Relevance score for term matching; `None` = no match.
    /// Higher is better. Empty terms match everything with score 0.
    pub fn score(&self, r: &FileRecord) -> Option<u32> {
        if self.terms.is_empty() {
            return Some(0);
        }
        let name = r.name();
        let mut total: u64 = 0;
        for term in &self.terms {
            if let Some(pos) = find_ci(name, term) {
                // Name match. Prefix matches and full-name matches score best.
                let base: u64 = if pos == 0 { 1 << 20 } else { 1 << 12 };
                let exact_bonus: u64 = if name.len() == term.len() { 1 << 24 } else { 0 };
                total += base + exact_bonus + 256u64.saturating_sub(pos.min(255) as u64);
                continue;
            }
            if find_ci(&r.path, term).is_some() {
                total += 1 << 6;
                continue;
            }
            return None;
        }
        Some(total.min(u32::MAX as u64) as u32)
    }
}

#[inline]
fn find_ci(haystack: &str, lower_needle: &str) -> Option<usize> {
    if lower_needle.is_empty() {
        return Some(0);
    }
    if haystack.is_ascii() && lower_needle.is_ascii() {
        let h = haystack.as_bytes();
        let n = lower_needle.as_bytes();
        return h.windows(n.len()).position(|w| w.eq_ignore_ascii_case(n));
    }
    haystack.to_lowercase().find(lower_needle)
}
#[inline]
fn starts_with_ci(haystack: &str, lower_prefix: &str) -> bool {
    if haystack.is_ascii() && lower_prefix.is_ascii() {
        haystack
            .as_bytes()
            .get(..lower_prefix.len())
            .is_some_and(|p| p.eq_ignore_ascii_case(lower_prefix.as_bytes()))
    } else {
        haystack.to_lowercase().starts_with(lower_prefix)
    }
}

/// Split input into tokens, respecting double quotes.
fn tokenize(input: &str) -> Vec<String> {
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

fn parse_size(spec: &str, q: &mut Query) {
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

fn parse_size_num(s: &str) -> Option<u64> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(path: &str, size: u64) -> FileRecord {
        FileRecord {
            path: path.into(),
            size,
            mtime: 0,
            mode: 0,
            kind: if path.ends_with('/') {
                FileKind::Dir
            } else {
                FileKind::File
            },
            fs: FsKind::Ext4,
            native_id: 0,
            native_parent: 0,
            source: 0,
        }
    }

    #[test]
    fn parses_filters() {
        let q = Query::parse("main ext:rs,toml size:>1k kind:file under:/src");
        assert_eq!(q.terms, vec!["main"]);
        assert_eq!(q.exts, vec!["rs", "toml"]);
        assert_eq!(q.min_size, Some(1024));
        assert_eq!(q.kinds, vec![FileKind::File]);
        assert_eq!(q.under.as_deref(), Some("/src"));
    }

    #[test]
    fn scores_name_over_path() {
        let q = Query::parse("config");
        let name_hit = rec("/home/u/projects/config.rs", 10);
        let path_hit = rec("/home/u/config/main.rs", 10);
        let s_name = q.score(&name_hit).unwrap();
        let s_path = q.score(&path_hit).unwrap();
        assert!(s_name > s_path);
        assert!(Query::parse("zzz").score(&name_hit).is_none());
    }

    #[test]
    fn size_ranges() {
        let q = Query::parse("size:1M..2M");
        assert_eq!(q.min_size, Some(1 << 20));
        assert_eq!(q.max_size, Some(2 << 20));
        assert!(q.passes_filters(&rec("/a/b", 1_500_000)));
        assert!(!q.passes_filters(&rec("/a/b", 3 << 20)));
    }

    #[test]
    fn quoted_terms() {
        let q = Query::parse("\"my doc\" ext:txt");
        assert_eq!(q.terms, vec!["my doc"]);
        assert_eq!(q.exts, vec!["txt"]);
    }
}
