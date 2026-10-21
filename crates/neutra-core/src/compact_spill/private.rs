use super::*;

#[cfg(windows)]
mod windows;

pub(super) fn prepare(path: &Path, output: &Path) -> io::Result<Vec<u8>> {
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let output = std::fs::canonicalize(parent)?.join(
        output
            .file_name()
            .ok_or_else(|| invalid("index output has no filename"))?,
    );
    let mut owner = b"NEUTRASEARCH-SPILL-1\0".to_vec();
    owner.extend_from_slice(output.as_os_str().as_encoded_bytes());
    match std::fs::symlink_metadata(path) {
        Ok(_) => cleanup(path, &owner)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    #[cfg(not(windows))]
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    #[cfg(not(windows))]
    builder.create(path)?;
    #[cfg(windows)]
    windows::create_directory(path)?;
    let mut marker = create_file(&path.join(".owner"))?;
    marker.write_all(&owner)?;
    marker.sync_all()?;
    Ok(owner)
}

pub(crate) fn create_file(path: &Path) -> io::Result<File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    options.open(path)
}

pub(super) fn cleanup(path: &Path, owner: &[u8]) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(invalid("refusing an unfamiliar spill directory"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            return Err(invalid(
                "spill directory must be private and owned by this account",
            ));
        }
    }
    let marker = path.join(".owner");
    let metadata = std::fs::symlink_metadata(&marker)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || std::fs::read(&marker)? != owner
    {
        return Err(invalid("spill directory does not belong to this index"));
    }
    std::fs::remove_dir_all(path)
}
