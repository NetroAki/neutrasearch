use crate::{FileKind, FileRecord, FsKind};
pub(super) fn rec(path: &str, size: u64) -> FileRecord {
    FileRecord {
        path: path.into(),
        size,
        mtime: size as i64,
        mode: 0,
        kind: FileKind::File,
        fs: FsKind::Btrfs,
        native_id: size + 100,
        native_parent: size + 10,
        source: 0,
        disk: 0,
    }
}
pub(super) fn dir(path: &str) -> FileRecord {
    FileRecord {
        path: path.into(),
        size: 0,
        mtime: 0,
        mode: 0,
        kind: FileKind::Dir,
        fs: FsKind::Btrfs,
        native_id: 0,
        native_parent: 0,
        source: 0,
        disk: 0,
    }
}
