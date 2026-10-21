#[cfg_attr(not(target_os = "linux"), allow(dead_code, unused_imports))]
use std::{
    path::{Path, PathBuf},
    process::Command,
};

#[cfg(not(target_os = "linux"))]
#[path = "transfer/copy.rs"]
mod copy;
#[cfg(not(target_os = "linux"))]
#[path = "transfer/rename.rs"]
mod rename;

pub(crate) fn transfer(paths: &[PathBuf], dest: &Path, cut: bool) -> Result<(), String> {
    if paths.is_empty() {
        return Err("no files to transfer".into());
    }
    let dest = if dest.is_dir() {
        dest.to_path_buf()
    } else {
        dest.parent().unwrap_or(Path::new(".")).to_path_buf()
    };
    for source in paths {
        let leaf = source
            .file_name()
            .ok_or_else(|| format!("path has no file name: {}", source.display()))?;
        let target = dest.join(leaf);
        if cut && same_path_parent(source, &dest) {
            continue;
        }
        if std::fs::symlink_metadata(&target).is_ok() {
            return Err(format!("destination already exists: {}", target.display()));
        }
        if std::fs::symlink_metadata(source)
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            if cut {
                move_path_noreplace(source, &target)?;
            } else {
                copy_path_noreplace(source, &target)?;
            }
            continue;
        }
        if cut {
            match rename_noreplace_io(source, &target) {
                Ok(()) => continue,
                Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
                    move_path_noreplace(source, &target)?;
                }
                Err(e) => return Err(format!("cannot move {}: {e}", source.display())),
            }
            continue;
        }
        copy_path_noreplace(source, &target)?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn staging_name(target: &Path) -> PathBuf {
    let leaf = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    target.with_file_name(format!(
        ".{leaf}.neutrasearch-{}-{nanos}.part",
        std::process::id()
    ))
}

fn same_path_parent(source: &Path, dest: &Path) -> bool {
    source.parent() == Some(dest)
}

fn copy_path_noreplace(source: &Path, target: &Path) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let directory = std::fs::symlink_metadata(source)
            .map_err(|error| error.to_string())?
            .is_dir();
        let refused = || {
            conflict_or(
                &format!(
                    "cannot copy {} without overwriting {}",
                    source.display(),
                    target.display()
                ),
                target,
            )
        };
        if std::fs::symlink_metadata(target).is_ok() {
            return Err(refused());
        }

        // Keep the payload inside an owner-only sibling directory so cp cannot
        // encounter a competing destination and silently skip it.
        let staged_dir = staging_name(target);
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&staged_dir)
            .map_err(|error| format!("cannot create private staging directory: {error}"))?;
        let staged = staged_dir.join("payload");
        let from = if directory {
            source.join(".")
        } else {
            source.to_path_buf()
        };
        let status = Command::new("timeout")
            .args([
                "1800",
                "cp",
                "-a",
                "--no-target-directory",
                "--reflink=auto",
                "--sparse=auto",
                "--preserve=all",
                "--",
            ])
            .arg(&from)
            .arg(&staged)
            .status();
        let status = match status {
            Ok(status) => status,
            Err(error) => {
                cleanup_staging(&staged_dir);
                return Err(format!(
                    "cannot start cp to copy {}: {error}",
                    source.display()
                ));
            }
        };
        if !status.success() {
            cleanup_staging(&staged_dir);
            return Err(refused());
        }
        match rename_noreplace_io(&staged, target) {
            Ok(()) => {
                cleanup_staging(&staged_dir);
                Ok(())
            }
            Err(_) => {
                cleanup_staging(&staged_dir);
                Err(refused())
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        copy::copy(source, target)
    }
}

#[cfg(target_os = "linux")]
fn cleanup_staging(path: &Path) {
    let _ = std::fs::remove_dir_all(path);
}

fn move_path_noreplace(source: &Path, target: &Path) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        match rename_noreplace_io(source, target) {
            Ok(()) => return Ok(()),
            Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {}
            Err(e) => return Err(format!("cannot move {}: {e}", source.display())),
        }
        if std::fs::symlink_metadata(target).is_ok() {
            return Err(conflict_or(
                &format!(
                    "cannot move {} without overwriting {}",
                    source.display(),
                    target.display()
                ),
                target,
            ));
        }
        let status = Command::new("timeout")
            .args(["1800", "mv", "--no-target-directory", "--update=none", "--"])
            .arg(source)
            .arg(target)
            .status()
            .map_err(|e| format!("cannot start mv to move {}: {e}", source.display()))?;
        // --update=none exits 0 when it skips, so confirm the source moved.
        if status.success() && std::fs::symlink_metadata(source).is_err() {
            return Ok(());
        }
        Err(conflict_or(
            &format!(
                "cannot move {} without overwriting {}",
                source.display(),
                target.display()
            ),
            target,
        ))
    }
    #[cfg(not(target_os = "linux"))]
    {
        match rename_noreplace_io(source, target) {
            Ok(()) => Ok(()),
            Err(e) => Err(format!("cannot move {}: {e}", source.display())),
        }
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code, unused_imports))]
fn conflict_or(message: &str, target: &Path) -> String {
    if target.exists() {
        message.to_owned()
    } else {
        format!("{message}: transfer tool refused the copy")
    }
}

#[cfg(target_os = "linux")]
fn rename_noreplace_io(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let a = std::ffi::CString::new(from.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid path"))?;
    let b = std::ffi::CString::new(to.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid path"))?;
    let rc = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            a.as_ptr(),
            libc::AT_FDCWD,
            b.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn rename_noreplace_io(from: &Path, to: &Path) -> std::io::Result<()> {
    rename::rename(from, to)
}

pub(super) fn rename_noreplace(from: &Path, to: &Path) -> Result<(), String> {
    rename_noreplace_io(from, to).map_err(|e| format!("cannot rename {}: {e}", from.display()))
}

#[cfg(test)]
#[path = "transfer/tests.rs"]
mod tests;
