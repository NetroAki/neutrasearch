use std::{fs, io, path::Path};

pub(super) fn copy(source: &Path, target: &Path) -> Result<(), String> {
    if fs::symlink_metadata(source)
        .map_err(|e| e.to_string())?
        .is_dir()
    {
        let origin = source.canonicalize().map_err(|e| e.to_string())?;
        let parent = target.parent().unwrap_or(Path::new("."));
        if parent
            .canonicalize()
            .map_err(|e| e.to_string())?
            .starts_with(&origin)
        {
            return Err("cannot copy a folder into itself".into());
        }
    }
    publish_staged(source, target).map_err(|e| {
        format!(
            "cannot copy {} to {}: {e}",
            source.display(),
            target.display()
        )
    })
}

/// Build the copy inside a private sibling directory and publish it with a
/// rename that refuses to replace anything, so the final name never holds a
/// partial tree and a failed copy leaves nothing behind.
fn publish_staged(source: &Path, target: &Path) -> io::Result<()> {
    let staging = super::staging_name(target);
    fs::create_dir(&staging)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&staging, fs::Permissions::from_mode(0o700))?;
    }
    let staged = staging.join("payload");
    let result =
        copy_entry(source, &staged).and_then(|()| super::rename_noreplace_io(&staged, target));
    super::cleanup_staging(&staging);
    result
}

fn copy_entry(source: &Path, target: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.is_symlink() {
        let link = fs::read_link(source)?;
        #[cfg(unix)]
        return std::os::unix::fs::symlink(link, target);
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            return if metadata.file_attributes() & 0x10 != 0 {
                std::os::windows::fs::symlink_dir(link, target)
            } else {
                std::os::windows::fs::symlink_file(link, target)
            };
        }
    }
    if metadata.is_dir() {
        fs::create_dir(target)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_entry(&entry.path(), &target.join(entry.file_name()))?;
        }
        return fs::set_permissions(target, metadata.permissions());
    }
    let mut input = fs::File::open(source)?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)?;
    let result = io::copy(&mut input, &mut output).and_then(|_| output.sync_all());
    drop(output);
    if let Err(error) = result {
        let _ = fs::remove_file(target);
        return Err(error);
    }
    fs::set_permissions(target, metadata.permissions())
}
