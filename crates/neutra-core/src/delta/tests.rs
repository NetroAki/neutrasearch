use super::*;
use crate::{FileKind, FsKind};
fn record(path: &str, size: u64) -> FileRecord {
    FileRecord {
        path: path.into(),
        size,
        mtime: 0,
        mode: 0,
        kind: FileKind::File,
        fs: FsKind::Btrfs,
        native_id: 0,
        native_parent: 0,
        source: 0,
        disk: 0,
    }
}
fn remove_log(path: &Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(lock_path(path));
}

mod cache;
#[cfg(unix)]
mod checkpoints;
mod integrity;
mod migration;
mod ownership;
#[cfg(unix)]
mod permissions;
mod replay;
mod shadows;
