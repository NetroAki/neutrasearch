use std::{
    fs,
    os::unix::fs::{DirBuilderExt, MetadataExt},
    path::{Path, PathBuf},
};

fn trash_name() -> String {
    format!(".Trash-{}", unsafe { libc::getuid() })
}

pub(super) fn filesystem_root(parent: &Path) -> Result<PathBuf, String> {
    let device = fs::metadata(parent).map_err(|e| e.to_string())?.dev();
    let mut root = parent;
    while let Some(ancestor) = root.parent() {
        if fs::metadata(ancestor).map_err(|e| e.to_string())?.dev() != device {
            break;
        }
        root = ancestor;
    }
    Ok(root.to_path_buf())
}

pub(super) fn prepare(root: &Path) -> Result<PathBuf, String> {
    let trash = root.join(trash_name());
    for directory in [&trash, &trash.join("files"), &trash.join("info")] {
        match fs::DirBuilder::new().mode(0o700).create(directory) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(format!("cannot create local Trash: {e}")),
        }
        private_directory(directory)?;
    }
    Ok(trash)
}

fn private_directory(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::getuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(format!("unsafe Trash directory: {}", path.display()));
    }
    Ok(())
}

pub(super) fn validate_token(token: &Path) -> Result<(PathBuf, PathBuf), String> {
    if !token.is_absolute() || token.extension().is_none_or(|s| s != "trashinfo") {
        return Err("invalid Trash restore information".into());
    }
    let info = token.parent().ok_or("invalid Trash location")?;
    let trash = info.parent().ok_or("invalid Trash location")?;
    let root = trash.parent().ok_or("invalid Trash location")?;
    if info.file_name().is_none_or(|s| s != "info")
        || trash.file_name().is_none_or(|s| s != trash_name().as_str())
    {
        return Err("invalid Trash location".into());
    }
    if fs::canonicalize(root).map_err(|e| e.to_string())? != root {
        return Err("invalid Trash filesystem root".into());
    }
    for directory in [trash, info, &trash.join("files")] {
        private_directory(directory)?;
    }
    Ok((root.to_path_buf(), trash.to_path_buf()))
}
