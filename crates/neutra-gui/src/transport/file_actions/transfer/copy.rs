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

/// macOS copies through cp, which uses copyfile(3) to keep resource forks,
/// extended attributes and ACLs that a byte copy would drop.
#[cfg(target_os = "macos")]
fn copy_entry(source: &Path, target: &Path) -> io::Result<()> {
    let status = std::process::Command::new("/bin/cp")
        .args(["-pR", "--"])
        .arg(source)
        .arg(target)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        let _ = fs::remove_dir_all(target).or_else(|_| fs::remove_file(target));
        Err(io::Error::other(format!("cp exited with {status}")))
    }
}

/// CopyFileW carries alternate data streams, attributes and timestamps, and
/// refuses to replace an existing target.
#[cfg(windows)]
fn copy_file(source: &Path, target: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    let wide = |p: &Path| -> Vec<u16> { p.as_os_str().encode_wide().chain(Some(0)).collect() };
    let (from, to) = (wide(source), wide(target));
    // SAFETY: both buffers are NUL-terminated and outlive the call.
    let ok = unsafe {
        windows_sys::Win32::Storage::FileSystem::CopyFileW(from.as_ptr(), to.as_ptr(), 1)
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Create the folder with the source's attributes. A folder that carries
/// named data streams cannot be copied faithfully, so refuse it.
#[cfg(windows)]
fn create_dir_like(source: &Path, target: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        CreateDirectoryExW, FindClose, FindFirstStreamW, FindStreamInfoStandard,
        WIN32_FIND_STREAM_DATA,
    };
    let wide = |p: &Path| -> Vec<u16> { p.as_os_str().encode_wide().chain(Some(0)).collect() };
    let (from, to) = (wide(source), wide(target));
    let mut data = WIN32_FIND_STREAM_DATA {
        StreamSize: 0,
        cStreamName: [0; 296],
    };
    // SAFETY: the path is NUL-terminated and data matches the requested level.
    let handle = unsafe {
        FindFirstStreamW(
            from.as_ptr(),
            FindStreamInfoStandard,
            (&mut data as *mut WIN32_FIND_STREAM_DATA).cast(),
            0,
        )
    };
    if handle != windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        // SAFETY: handle came from a successful FindFirstStreamW.
        unsafe { FindClose(handle) };
        return Err(io::Error::other(
            "the folder has alternate data streams that a copy cannot carry",
        ));
    }
    // SAFETY: both buffers are NUL-terminated and outlive the call.
    if unsafe { CreateDirectoryExW(from.as_ptr(), to.as_ptr(), std::ptr::null()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
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
        #[cfg(windows)]
        create_dir_like(source, target)?;
        #[cfg(not(windows))]
        fs::create_dir(target)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_entry(&entry.path(), &target.join(entry.file_name()))?;
        }
        return fs::set_permissions(target, metadata.permissions());
    }
    #[cfg(windows)]
    return copy_file(source, target);
    #[cfg(not(windows))]
    copy_regular_file(source, target, &metadata)
}

#[cfg(not(any(windows, target_os = "macos")))]
fn copy_regular_file(source: &Path, target: &Path, metadata: &fs::Metadata) -> io::Result<()> {
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
