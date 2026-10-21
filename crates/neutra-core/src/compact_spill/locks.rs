use fs2::FileExt;
use std::io;
use std::path::Path;

use crate::compact::invalid;

/// Serialize the rebuild lock from `CompactIndex::rebuild` so both build
/// paths share one gate and one obsolete-WAL removal. Lives here (rather
/// than beside the builders) because the spill directory it protects is
/// created here.
pub(crate) fn with_rebuild_lock<T>(
    path: &Path,
    build: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    with_writer_lock(path, || {
        let mut delta = path.to_path_buf();
        delta.set_extension("delta");
        let built = build()?;
        // Streamed publication installs a fresh WAL bound to the new base. Keep
        // that WAL; legacy/test rebuilds still remove obsolete generation logs.
        let published_generation = crate::CompactIndex::generation_on_disk(path).ok();
        let wal_matches = published_generation
            .is_some_and(|generation| crate::DeltaIndex::open_snapshot(&delta, generation).is_ok());
        if !wal_matches {
            match std::fs::remove_file(&delta) {
                Ok(()) => crate::compact_build::sync_parent(&delta)?,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok(built)
    })
}

pub(crate) fn with_writer_lock<T>(
    path: &Path,
    build: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    let mut delta = path.to_path_buf();
    delta.set_extension("delta");
    let lock_path = crate::compact::suffix_path(&delta, ".lock");
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let lock = options.open(lock_path)?;
    let metadata = lock.metadata()?;
    if !metadata.is_file() {
        return Err(invalid("compact rebuild lock is not a regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 || metadata.mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "compact rebuild lock must be private and single-linked",
            ));
        }
    }
    match lock.try_lock_exclusive() {
        Ok(()) => {}
        Err(error) => {
            // Windows reports LockFileEx contention as ERROR_LOCK_VIOLATION
            // (33), which fs2 exposes as Uncategorized rather than WouldBlock.
            #[cfg(windows)]
            if error.raw_os_error() == Some(33) {
                return Err(io::Error::new(io::ErrorKind::WouldBlock, error));
            }
            return Err(error);
        }
    }
    build()
}
