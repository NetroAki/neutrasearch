//! Content checks for hardlinks, whose shared ctime changes during unlink.

use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};

pub(super) fn read(path: &Path, before: &fs::Metadata) -> io::Result<[u8; 32]> {
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)?;
    let opened = file.metadata()?;
    if !opened.is_file() || !same_file_state(before, &opened) {
        return Err(io::Error::other("file changed while taking move snapshot"));
    }

    let mut digest = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }

    let after = file.metadata()?;
    if !same_file_state(&opened, &after) {
        return Err(io::Error::other("file changed while taking move snapshot"));
    }
    Ok(digest.finalize().into())
}

pub(super) fn matches(path: &Path, expected: &[u8; 32]) -> bool {
    fs::symlink_metadata(path)
        .and_then(|metadata| read(path, &metadata))
        .is_ok_and(|actual| &actual == expected)
}

fn same_file_state(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    a.dev() == b.dev()
        && a.ino() == b.ino()
        && a.size() == b.size()
        && a.mtime() == b.mtime()
        && a.mtime_nsec() == b.mtime_nsec()
        && a.ctime() == b.ctime()
        && a.ctime_nsec() == b.ctime_nsec()
}
