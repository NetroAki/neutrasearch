use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

mod metadata;
mod storage;

static NEXT: AtomicU64 = AtomicU64::new(0);

pub(super) fn trash(path: &Path) -> Result<PathBuf, String> {
    let leaf = path.file_name().ok_or("cannot trash a filesystem root")?;
    let parent =
        fs::canonicalize(path.parent().ok_or("file has no parent")?).map_err(|e| e.to_string())?;
    let source = parent.join(leaf);
    let root = storage::filesystem_root(&parent)?;
    move_to_trash(&source, &root)
}

fn move_to_trash(source: &Path, root: &Path) -> Result<PathBuf, String> {
    let trash = storage::prepare(root)?;
    if source == root || source.starts_with(&trash) {
        return Err("cannot trash the filesystem root or Trash itself".into());
    }
    let relative = source.strip_prefix(root).map_err(|e| e.to_string())?;
    let contents = metadata::encode(relative)?;
    let (token, mut info, destination) = reserve(&trash)?;
    if let Err(error) = info
        .write_all(contents.as_bytes())
        .and_then(|_| info.sync_all())
    {
        let _ = fs::remove_file(&token);
        return Err(format!("cannot save Trash restore information: {error}"));
    }
    if let Err(error) = super::super::rename_noreplace(source, &destination) {
        let _ = fs::remove_file(&token);
        return Err(error);
    }
    Ok(token)
}

fn reserve(trash: &Path) -> Result<(PathBuf, fs::File, PathBuf), String> {
    for _ in 0..100 {
        let id = format!(
            "neutrasearch-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let token = trash.join("info").join(format!("{id}.trashinfo"));
        let destination = trash.join("files").join(&id);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&token)
        {
            Ok(file) => return Ok((token, file, destination)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("cannot create Trash restore information: {e}")),
        }
    }
    Err("cannot reserve a unique Trash location".into())
}

pub(super) fn restore(token: &Path) -> Result<PathBuf, String> {
    let (root, trash) = storage::validate_token(token)?;
    let original = root.join(metadata::read(token)?);
    let id = token.file_stem().ok_or("invalid Trash item")?;
    super::super::rename_noreplace(&trash.join("files").join(id), &original)?;
    fs::remove_file(token).map_err(|e| {
        format!(
            "File restored to {}, but its Trash metadata could not be removed: {e}",
            original.display()
        )
    })?;
    Ok(original)
}

#[cfg(test)]
mod tests;
