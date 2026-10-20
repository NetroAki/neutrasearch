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

mod syntax;
pub(crate) use syntax::safe_absolute_path;
use syntax::{parse_size, path_is_under, path_is_under_ci, tokenize};

#[cfg(test)]
mod tests;
