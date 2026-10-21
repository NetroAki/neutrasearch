use super::*;

pub(crate) fn make_record(
    path: &Path,
    stat: &libc::stat,
    parent: u64,
    mount: &MountInfo,
    source: u32,
) -> Option<FileRecord> {
    let kind = match stat.st_mode & libc::S_IFMT {
        libc::S_IFDIR => FileKind::Dir,
        libc::S_IFLNK => FileKind::Symlink,
        libc::S_IFREG => FileKind::File,
        _ => FileKind::Other,
    };
    Some(FileRecord {
        path: path.to_str()?.to_owned().into_boxed_str(),
        size: stat.st_size.max(0) as u64,
        disk: FileRecord::allocated_bytes((stat.st_blocks.max(0) as u64).saturating_mul(512)),
        mtime: stat.st_mtime,
        mode: stat.st_mode,
        kind,
        fs: mount.fs.clone(),
        native_id: stat.st_ino,
        native_parent: parent,
        source,
    })
}
