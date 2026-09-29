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
    /// Apparent size in bytes (what `ls -l` shows).
    pub size: u64,
    /// Bytes actually occupying disk (sparse holes excluded). Zero means
    /// unknown; readers must fall back to `size` via `disk_bytes`.
    #[serde(default)]
    pub disk: u64,
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
    /// On-disk footprint for disk-usage views.
    pub fn disk_bytes(&self) -> u64 {
        if self.disk == 0 { self.size } else { self.disk }
    }
     /// Decode records in the current layout. Layout selection is always driven
     /// by the enclosing container explicit version, never by probing: a
     /// payload without disk shares a bincode prefix with one that has it,
     /// so content sniffing silently yields shifted fields.
     pub fn decode(bytes: &[u8]) -> bincode::Result<Vec<Self>> {
         bincode::deserialize(bytes)
     }
     /// Decode records in the frozen pre-disk layout. The caller selected
     /// this branch from its container version. Missing disk values read
     /// back as zero and fall back to size via disk_bytes.
     pub fn decode_old(bytes: &[u8]) -> bincode::Result<Vec<Self>> {
         Ok(bincode::deserialize::<Vec<OldRecord>>(bytes)?
             .into_iter()
             .map(FileRecord::from)
             .collect())
     }
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

/// Frozen pre-`disk` layout, for payloads written by older binaries.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct OldRecord {
    path: Box<str>,
    size: u64,
    mtime: i64,
    mode: u32,
    kind: FileKind,
    fs: crate::mounts::FsKind,
    #[serde(default)]
    native_id: u64,
    #[serde(default)]
    native_parent: u64,
    source: u32,
}

impl From<OldRecord> for FileRecord {
    fn from(old: OldRecord) -> Self {
        Self {
            path: old.path,
            size: old.size,
            disk: 0,
            mtime: old.mtime,
            mode: old.mode,
            kind: old.kind,
            fs: old.fs,
            native_id: old.native_id,
            native_parent: old.native_parent,
            source: old.source,
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

#[cfg(test)]
mod tests {
    use super::*;
    fn record(size: u64, disk: u64) -> FileRecord {
        FileRecord {
            path: "/a".into(),
            size,
            disk,
            mtime: 0,
            mode: 0,
            kind: FileKind::File,
            fs: crate::mounts::FsKind::Unsupported("t".into()),
            native_id: 0,
            native_parent: 0,
            source: 0,
        }
    }
    #[test]
    fn disk_falls_back_and_old_payloads_decode() {
        assert_eq!(record(10, 4).disk_bytes(), 4);
        assert_eq!(record(10, 0).disk_bytes(), 10);
        let old = OldRecord {
            path: "/b".into(),
            size: 7,
            mtime: 1,
            mode: 2,
            kind: FileKind::Dir,
            fs: crate::mounts::FsKind::Unsupported("t".into()),
            native_id: 3,
            native_parent: 4,
            source: 5,
        };
         // Old-layout bytes decode through the old branch with disk unknown.
         let bytes = bincode::serialize(&vec![old]).unwrap();
         let decoded = FileRecord::decode_old(&bytes).unwrap();
         assert_eq!(decoded.len(), 1);
         assert_eq!(decoded[0].size, 7);
         assert_eq!(decoded[0].disk, 0);
         assert_eq!(decoded[0].disk_bytes(), 7);
         // Current-layout bytes decode through the current branch intact.
         let bytes = bincode::serialize(&vec![record(9, 3)]).unwrap();
         let decoded = FileRecord::decode(&bytes).unwrap();
         assert_eq!(decoded[0].size, 9);
         assert_eq!(decoded[0].disk_bytes(), 3);
    }
}
