//! Zero-copy view of an encoded record. Scans score the borrowed path first
//! and allocate a `FileRecord` only for the few records that match.

use crate::mounts::FsKind;
use crate::types::{FileKind, FileRecord};
use serde::Deserialize;

/// Field order must match `FileRecord` exactly: bincode is positional.
#[derive(Deserialize)]
pub(crate) struct RecordRef<'a> {
    #[serde(borrow)]
    pub(crate) path: &'a str,
    size: u64,
    disk: u64,
    mtime: i64,
    mode: u32,
    kind: FileKind,
    fs: FsKind,
    native_id: u64,
    native_parent: u64,
    source: u32,
}

impl RecordRef<'_> {
    pub(crate) fn from_record(record: &FileRecord) -> RecordRef<'_> {
        RecordRef {
            path: &record.path,
            size: record.size,
            disk: record.disk,
            mtime: record.mtime,
            mode: record.mode,
            kind: record.kind,
            fs: record.fs.clone(),
            native_id: record.native_id,
            native_parent: record.native_parent,
            source: record.source,
        }
    }

    pub(crate) fn to_record(&self) -> FileRecord {
        FileRecord {
            path: self.path.into(),
            size: self.size,
            disk: self.disk,
            mtime: self.mtime,
            mode: self.mode,
            kind: self.kind,
            fs: self.fs.clone(),
            native_id: self.native_id,
            native_parent: self.native_parent,
            source: self.source,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn borrowed_view_round_trips_the_owned_encoding() {
        let record = FileRecord {
            path: "/music/stems/Calling.wav".into(),
            size: 12,
            disk: 4096,
            mtime: 1_700_000_000,
            mode: 0o100644,
            kind: FileKind::File,
            fs: FsKind::Btrfs,
            native_id: 7,
            native_parent: 3,
            source: 2,
        };
        let bytes = bincode::serialize(&vec![record.clone(), record.clone()]).unwrap();
        let views: Vec<RecordRef> = bincode::deserialize(&bytes).unwrap();
        assert_eq!(views.len(), 2);
        let back = views[1].to_record();
        assert_eq!(
            bincode::serialize(&back).unwrap(),
            bincode::serialize(&record).unwrap()
        );
        let encoded = bincode::serialize(&record).unwrap();
        let single: RecordRef = bincode::deserialize(&encoded).unwrap();
        assert_eq!(single.path, &*record.path);
    }
}
