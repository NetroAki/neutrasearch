use crate::mounts::FsKind;
use serde::{Deserialize, Serialize};

/// What a record points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileKind {
    File,
    Dir,
    Symlink,
    /// Sockets, fifos, devices, or anything the scanner could not classify.
    Other,
}

/// One indexed filesystem entry. Path is the single source of truth; the file
/// name is derived from it (`rsplit('/')`) so we never store it twice.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRecord {
    /// Absolute path as seen from the scanning machine (for remote sources,
    /// the path is prefixed with the remote mount alias by the client).
    pub path: Box<str>,
    pub size: u64,
    /// Unix seconds; 0 when the source filesystem did not provide one.
    pub mtime: i64,
    /// Unix mode bits where meaningful (0 on NTFS).
    pub mode: u32,
    pub kind: FileKind,
    /// Which filesystem lane produced this record.
    pub fs: FsKind,
    /// Filesystem-native stable object identity (inode/FRN where available).
    #[serde(default)]
    pub native_id: u64,
    /// Filesystem-native parent identity; required for rename reconciliation.
    #[serde(default)]
    pub native_parent: u64,
    /// Identifies the index source: 0 = local, otherwise a remote-source id
    /// assigned by the client when merging helper indexes.
    pub source: u32,
}

impl FileRecord {
    /// File name component of the path.
    pub fn name(&self) -> &str {
        match self.path.rfind('/') {
            Some(i) => &self.path[i + 1..],
            None => &self.path,
        }
    }

    /// Extension, lowercased, without the dot. Empty for dotfiles/no ext.
    pub fn extension(&self) -> &str {
        let name = self.name();
        match name.rfind('.') {
            // ".gitignore" has no extension; "a." has none either.
            Some(0) | None => "",
            Some(i) if i == name.len() - 1 => "",
            Some(i) => &name[i + 1..],
        }
    }
}

/// Per-scan statistics emitted by every lane so the UI/MCP can prove speed.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScanStats {
    pub records: u64,
    pub dirs: u64,
    pub files: u64,
    pub bytes_read: u64,
    pub wall_ms: u64,
    /// Human-readable lane detail, e.g. "MFT records: 412998, fragmented: no".
    pub detail: String,
}
