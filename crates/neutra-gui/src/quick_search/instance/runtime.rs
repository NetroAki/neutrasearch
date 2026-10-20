use std::{io, os::unix::fs::MetadataExt, path::PathBuf};

pub(super) fn private_directory(path: PathBuf) -> io::Result<PathBuf> {
    let metadata = std::fs::symlink_metadata(&path)?;
    if !path.is_absolute()
        || !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "quick-search runtime must be a private directory owned by this user",
        ));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, DirBuilderExt, PermissionsExt};

    #[test]
    fn accepts_private_runtime_and_rejects_public_or_linked_directories() {
        let root = std::env::temp_dir().join(format!("neutra-runtime-{}", std::process::id()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .unwrap();
        assert_eq!(private_directory(root.clone()).unwrap(), root);
        let link = root.join("link");
        symlink(&root, &link).unwrap();
        assert!(private_directory(link).is_err());
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(private_directory(root.clone()).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
