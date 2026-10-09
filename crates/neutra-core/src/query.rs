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
//! Callers may additionally set `regex` (full pattern per term semantics),
//! `case_sensitive`, `whole_word`, `fold_accents`, and `match_fields` to
//! control where and how terms match.

use crate::mounts::FsKind;
use crate::types::{FileKind, FileRecord};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortKey {
    Relevance,
    NameAsc,
    NameDesc,
    SizeDesc,
    SizeAsc,
    MtimeDesc,
    MtimeAsc,
    PathAsc,
    PathDesc,
}

/// Which part of a record the text terms must match.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatchFields {
    Name,
    #[default]
    NameAndPath,
    Path,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Query {
    /// Substrings that must ALL match (location chosen by `match_fields`).
    /// Stored verbatim; case handling is `case_sensitive`.
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
    #[serde(default)]
    pub exclude_roots: Vec<String>,
    /// When set, every term position is filled by one regular expression that
    /// must match the selected fields. Stored as a pattern string so the
    /// query stays wire-serializable; the engine compiles it once per search.
    #[serde(default)]
    pub regex: Option<String>,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub match_fields: MatchFields,
    /// When true, each term must span whole words (bounded by string ends
    /// or non-alphanumeric characters), not substrings.
    #[serde(default)]
    pub whole_word: bool,
    /// When true, accents fold away before comparing (`cafe` finds `caf\u{e9}`).
    /// Default false preserves exact-accent matching.
    #[serde(default)]
    pub fold_accents: bool,
    /// When true, only programs pass: an executable mode bit or a program
    /// extension (see `EXEC_EXTS`). Extension lists stay empty so `+x`
    /// binaries without an extension still match.
    #[serde(default)]
    pub executable_only: bool,
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
            exclude_roots: Vec::new(),
            regex: None,
            case_sensitive: false,
            whole_word: false,
            fold_accents: false,
            executable_only: false,
            match_fields: MatchFields::default(),
            sort: SortKey::Relevance,
            limit: 1000,
        }
    }
}

/// Shared file-type extension groups for kind preset buttons. Kept here so
/// the GUI, MCP, and query CLI map presets identically.
pub const AUDIO_EXTS: &[&str] = &[
    "mp3", "wav", "aif", "aiff", "flac", "ogg", "oga", "opus", "m4a", "aac", "wma", "mid", "midi",
    "amr", "alac", "ape", "wv", "mka", "rx2", "rex", "dsf", "dff",
];
pub const IMAGE_EXTS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "webp", "svg", "bmp", "ico", "tif", "tiff", "heic", "heif",
    "avif", "jxl", "psd", "xcf", "raw", "cr2", "nef", "arw", "dng", "orf", "rw2",
];
pub const VIDEO_EXTS: &[&str] = &[
    "mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v", "mpg", "mpeg", "3gp", "ts", "m2ts",
    "vob", "ogv",
];
pub const ARCHIVE_EXTS: &[&str] = &[
    "zip", "7z", "rar", "tar", "gz", "bz2", "xz", "zst", "lz4", "cab", "iso", "img", "dmg", "pkg",
    "deb", "rpm", "apk", "jar", "cbz", "cbr",
];
pub const DOC_EXTS: &[&str] = &[
    "pdf", "doc", "docx", "odt", "rtf", "txt", "md", "markdown", "rst", "tex", "epub", "mobi",
    "azw", "fb2", "djvu", "xls", "xlsx", "ods", "csv", "tsv", "ppt", "pptx", "odp", "log", "org",
    "nfo",
];
/// Program extensions. Records with an executable mode bit also pass (see
/// `is_executable`), so extensionless `+x` binaries are not excluded.
pub const EXEC_EXTS: &[&str] = &[
    "exe", "msi", "bat", "cmd", "com", "scr", "ps1", "vbs", "sh", "bash", "zsh", "fish", "run",
    "bin", "appimage", "deb", "rpm", "apk", "jar", "msix", "app", "gadget",
];

/// True for programs: executable mode bit set, or a program extension.
/// NTFS records carry `mode == 0`, so they match by extension only.
pub fn is_executable(r: &FileRecord) -> bool {
    if r.mode & 0o111 != 0 {
        return true;
    }
    let ext = r.extension();
    EXEC_EXTS
        .iter()
        .any(|want| ext.len() == want.len() && ext.eq_ignore_ascii_case(want))
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
                q.terms.push(tok);
            }
        }
        q
    }

    /// Compile the text matcher once per search. Invalid regex patterns are
    /// reported instead of silently matching nothing. The matcher owns a
    /// copy of the query so callers (e.g. the GUI) may cache it.
    pub fn matcher(&self) -> std::io::Result<crate::matcher::QueryMatcher> {
        crate::matcher::QueryMatcher::new(self.clone())
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
        if self.executable_only && !is_executable(r) {
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
            if r.disk_bytes() < min {
                return false;
            }
        }
        if let Some(max) = self.max_size {
            if r.disk_bytes() > max {
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
        if self.exclude_roots.iter().any(|root| {
            if self.scope_case_sensitive {
                path_is_under(&r.path, root)
            } else {
                path_is_under_ci(&r.path, root)
            }
        }) {
            return false;
        }
        true
    }

    /// Relevance score for term matching; `None` = no match.
    /// Higher is better. Empty terms match everything with score 0.
    /// Compiles the matcher on every call; engines should use `matcher()`
    /// once per search instead.
    pub fn score(&self, r: &FileRecord) -> Option<u32> {
        self.matcher().ok()?.score(r)
    }
}

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
            disk: 0,
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

    #[test]
    fn regex_mode_matches_like_terms() {
        let mut q = Query::parse("");
        q.regex = Some(r"inv[o0]ice.*\.pdf$".into());
        let hit = rec("/home/u/Invoice-2026.pdf", 1);
        let miss = rec("/home/u/invoice.txt", 1);
        let matcher = q.matcher().unwrap();
        assert!(matcher.score(&hit).is_some());
        assert!(matcher.score(&miss).is_none());

        q.regex = Some("([".into());
        assert!(q.matcher().is_err());
    }

    #[test]
    fn case_sensitive_terms_reject_other_case() {
        let mut q = Query::parse("Report");
        let upper = rec("/home/u/Report.docx", 1);
        let lower = rec("/home/u/report.docx", 1);
        assert!(q.score(&upper).is_some());
        assert!(q.score(&lower).is_some());
        q.case_sensitive = true;
        assert!(q.score(&upper).is_some());
        assert!(q.score(&lower).is_none());
    }

    #[test]
    fn match_fields_restrict_where_terms_match() {
        let mut q = Query::parse("projects");
        let name_hit = rec("/home/u/Projects", 1);
        let path_hit = rec("/home/u/projects/readme.md", 1);
        assert!(q.score(&name_hit).is_some());
        assert!(q.score(&path_hit).is_some());
        q.match_fields = MatchFields::Name;
        assert!(q.score(&name_hit).is_some());
        assert!(q.score(&path_hit).is_none());
        q.match_fields = MatchFields::Path;
        assert!(q.score(&path_hit).is_some());
    }

    #[test]
    fn executable_and_whole_word_options_narrow_matches() {
        let mut q = Query::parse("call");
        q.whole_word = true;
        // Name-only default in the GUI aside, the engine honors the flag:
        // "calling" is not a whole-word hit for "call".
        assert!(q
            .matcher()
            .unwrap()
            .score(&rec("/m/the calling.wav", 1))
            .is_none());
        assert!(q
            .matcher()
            .unwrap()
            .score(&rec("/m/the call.wav", 1))
            .is_some());
        let mut exe = Query::parse("");
        exe.executable_only = true;
        assert!(!exe.passes_filters(&rec("/usr/bin/tool", 1)));
        assert!(exe.passes_filters(&rec("/opt/app/setup.exe", 1)));
    }
}
