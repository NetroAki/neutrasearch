use super::*;

#[cfg(unix)]
pub(super) fn file_identity_from_metadata(metadata: &Metadata) -> io::Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(unix)]
pub(super) fn file_identity(path: &Path) -> io::Result<(u64, u64)> {
    file_identity_from_metadata(&File::open(path)?.metadata()?)
}

#[cfg(not(unix))]
pub(super) fn file_identity_from_metadata(_: &Metadata) -> io::Result<(u64, u64)> {
    Ok((0, 0))
}

pub(super) fn lock_path(path: &Path) -> PathBuf {
    let mut lock = path.as_os_str().to_os_string();
    lock.push(".lock");
    lock.into()
}
pub(super) fn truncate_private(path: &Path, len: u64) -> io::Result<()> {
    reject_symlink(path)?;
    let mut options = OpenOptions::new();
    options.create(true).write(true).truncate(false);
    configure_private_options(&mut options);
    let file = options.open(path)?;
    validate_private_file(&file)?;
    file.set_len(len)?;
    file.sync_data()
}
pub(super) fn open_reset_writer(path: &Path, generation: u64) -> io::Result<BufWriter<File>> {
    truncate_private(path, 0)?;
    let mut file = open_append_private(path)?;
    file.write_all(MAGIC)?;
    file.write_all(&generation.to_le_bytes())?;
    file.sync_all()?;
    Ok(BufWriter::with_capacity(64 * 1024, file))
}
pub(super) fn open_append_private(path: &Path) -> io::Result<File> {
    reject_symlink(path)?;
    let mut options = OpenOptions::new();
    // Windows append access alone cannot truncate a torn tail with SetEndOfFile.
    // Keep append semantics for frames while also requesting general write access.
    options.create(true).append(true).write(true).read(true);
    configure_private_options(&mut options);
    let file = options.open(path)?;
    validate_private_file(&file)?;
    Ok(file)
}

pub(super) fn configure_private_options(options: &mut OpenOptions) {
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
}

pub(super) fn reject_symlink(path: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("refusing symlinked private state file {}", path.display()),
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub(super) fn validate_private_file(file: &File) -> io::Result<()> {
    validate_private_metadata(&file.metadata()?)
}

pub(super) fn validate_private_metadata(metadata: &Metadata) -> io::Result<()> {
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "private state path is not a regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.mode() & 0o077 != 0 || metadata.nlink() != 1 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "private state file must be owner-only and have exactly one link",
            ));
        }
    }
    Ok(())
}
