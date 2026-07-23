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
    /// Lowercased path prefix supplied by the query language.
    pub under: Option<String>,
    /// Trusted caller-injected path scopes, ORed together before ranking and limiting.
    #[serde(default)]
    pub scope_roots: Vec<String>,
    /// Security-sensitive callers use host filesystem case semantics for scopes.
    #[serde(default)]
    pub scope_case_sensitive: bool,
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
            scope_roots: Vec::new(),
            scope_case_sensitive: false,
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
        if !safe_absolute_path(&r.path) {
            return false;
        }
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
            if !path_is_under_ci(&r.path, under) {
                return false;
            }
        }
        if !self.scope_roots.is_empty()
            && !self.scope_roots.iter().any(|root| {
                if self.scope_case_sensitive {
                    path_is_under(&r.path, root)
                } else {
                    path_is_under_ci(&r.path, root)
                }
            })
        {
            return false;
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
fn safe_absolute_path(path: &str) -> bool {
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
fn path_is_under(path: &str, root: &str) -> bool {
    path.strip_prefix(root).is_some_and(|rest| {
        rest.is_empty()
            || root.ends_with('/')
            || root.ends_with('\\')
            || rest.starts_with('/')
            || rest.starts_with('\\')
    })
}

#[inline]
fn path_is_under_ci(path: &str, lower_root: &str) -> bool {
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
    fn under_filter_respects_path_component_boundaries() {
        let q = Query::parse("under:/home/a");
        assert!(q.passes_filters(&rec("/home/a/file.txt", 1)));
        assert!(q.passes_filters(&rec("/home/a", 1)));
        assert!(!q.passes_filters(&rec("/home/ab/file.txt", 1)));

        let windows = Query::parse(r"under:C:\Users\A");
        assert!(windows.passes_filters(&rec(r"c:\Users\A\file.txt", 1)));
        assert!(!windows.passes_filters(&rec(r"c:\Users\AB\file.txt", 1)));
    }

    #[test]
    fn trusted_scope_roots_are_orred_with_component_boundaries() {
        let mut q = Query::parse("");
        q.scope_roots = vec!["/allowed/a".into(), "/allowed/b".into()];
        assert!(q.passes_filters(&rec("/allowed/a/file.txt", 1)));
        assert!(q.passes_filters(&rec("/allowed/b/file.txt", 1)));
        assert!(!q.passes_filters(&rec("/allowed/ab/file.txt", 1)));
        assert!(!q.passes_filters(&rec("/denied/file.txt", 1)));
        assert!(!q.passes_filters(&rec("/allowed/a/../secret.txt", 1)));
    }

    #[test]
    fn trusted_windows_scope_accepts_native_or_portable_separators() {
        let mut query = Query::parse("");
        query.scope_roots = vec![r"C:\Users\Alex".into()];
        assert!(query.passes_filters(&rec("C:/Users/Alex/report.txt", 1)));
        assert!(query.passes_filters(&rec(r"c:\users\alex\report.txt", 1)));
        assert!(!query.passes_filters(&rec("C:/Users/Alexander/report.txt", 1)));
    }

    #[test]
    fn trusted_scope_can_enforce_case_sensitive_host_semantics() {
        let mut query = Query::parse("");
        query.scope_roots = vec!["/Users/Alice".into()];
        assert!(query.passes_filters(&rec("/users/alice/file.txt", 1)));
        query.scope_case_sensitive = true;
        assert!(query.passes_filters(&rec("/Users/Alice/file.txt", 1)));
        assert!(!query.passes_filters(&rec("/users/alice/file.txt", 1)));
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
