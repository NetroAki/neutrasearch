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
        if directory {
            std::fs::create_dir(target)
                .map_err(|error| format!("cannot create destination folder: {error}"))?;
        }
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
        if !directory && std::fs::symlink_metadata(target).is_ok() {
            return Err(refused());
        }
        // A file is copied beside its destination under a private name and then
        // renamed into place without replacing, so a destination created while
        // the copy runs is reported instead of skipped silently.
        let staged = (!directory).then(|| staging_name(target));
        let copy_target = staged.as_deref().unwrap_or(target);
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
                "--update=none",
                "--",
            ])
            .arg(&from)
            .arg(copy_target)
            .status()
            .map_err(|e| format!("cannot start cp to copy {}: {e}", source.display()))?;
        let Some(staged) = staged else {
            return if status.success() {
                Ok(())
            } else {
                Err(refused())
            };
        };
        if !status.success() {
            let _ = std::fs::remove_file(&staged);
            return Err(refused());
        }
        match rename_noreplace_io(&staged, target) {
            Ok(()) => Ok(()),
            Err(_) => {
                let _ = std::fs::remove_file(&staged);
                Err(refused())
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        copy::copy(source, target)
    }
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
