use super::*;

pub(crate) fn temp_path(path: &Path) -> PathBuf {
    suffix_path(path, ".new")
}

pub(super) fn publish_copy(staged: &Path, destination: &Path) -> io::Result<()> {
    // A failed copy must not poison every later recovery attempt. Each
    // publication owns its private temporary file; old attempts stay intact.
    let temporary = suffix_path(destination, &format!(".publish-{}.new", new_generation()));
    // Publication runs under the store write lock. A hard link of the staged
    // file is O(1); the byte copy is only for filesystems without links.
    let result = (|| {
        flush_file(staged)?;
        if std::fs::hard_link(staged, &temporary).is_ok() {
            return Ok(());
        }
        let mut copy = open_private(&temporary)?;
        let mut source = File::open(staged)?;
        std::io::copy(&mut source, &mut copy)?;
        copy.flush()?;
        copy.get_ref().sync_all()?;
        Ok(())
    })();
    let result = result.and_then(|()| replace_file(&temporary, destination));
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result?;
    sync_parent(destination)
}

pub(crate) fn clear_stale_marker(path: &Path) -> io::Result<()> {
    let stale = suffix_path(path, ".stale");
    match std::fs::remove_file(stale) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(not(windows))]
pub(crate) fn replace_file(temp: &Path, path: &Path) -> io::Result<()> {
    std::fs::rename(temp, path)
}

#[cfg(unix)]
pub(crate) fn sync_parent(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

pub(crate) fn open_private(path: &Path) -> io::Result<BufWriter<File>> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true).read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    Ok(BufWriter::with_capacity(1024 * 1024, options.open(path)?))
}

/// Windows refuses FlushFileBuffers on a read-only handle.
fn flush_file(path: &Path) -> io::Result<()> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(cfg!(windows))
        .open(path)?
        .sync_all()
}
