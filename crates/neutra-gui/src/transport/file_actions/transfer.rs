use std::{
    path::{Path, PathBuf},
    process::Command,
};

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
        let source = if directory {
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
                "--update=none-fail",
                "--",
            ])
            .arg(&source)
            .arg(target)
            .status()
            .map_err(|e| format!("cannot start cp to copy {}: {e}", source.display()))?;
        if status.success() {
            return Ok(());
        }
        Err(conflict_or(
            &format!(
                "cannot copy {} without overwriting {}",
                source.display(),
                target.display()
            ),
            target,
        ))
    }
    #[cfg(not(target_os = "linux"))]
    {
        copy_file_noreplace(source, target)
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
        let status = Command::new("timeout")
            .args([
                "1800",
                "mv",
                "--no-target-directory",
                "--update=none-fail",
                "--",
            ])
            .arg(source)
            .arg(target)
            .status()
            .map_err(|e| format!("cannot start mv to move {}: {e}", source.display()))?;
        if status.success() {
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
        match rename_noreplace_io(source, &target) {
            Ok(()) => Ok(()),
            Err(e) => Err(format!("cannot move {}: {e}", source.display())),
        }
    }
}

fn conflict_or(message: &str, target: &Path) -> String {
    if target.exists() {
        message.to_owned()
    } else {
        format!("{message}: transfer tool refused the copy")
    }
}

#[cfg(not(target_os = "linux"))]
fn copy_file_noreplace(source: &Path, target: &Path) -> Result<(), String> {
    use std::io;
    let mut input = std::fs::File::open(source)
        .map_err(|e| format!("cannot read {}: {e}", source.display()))?;
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|e| {
            format!(
                "cannot create {} without overwriting: {e}",
                target.display()
            )
        })?;
    if let Err(e) = io::copy(&mut input, &mut output) {
        drop(output);
        let _ = std::fs::remove_file(target);
        return Err(format!("copy failed: {e}"));
    }
    Ok(())
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
    if to.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "destination exists",
        ));
    }
    std::fs::rename(from, to)
}

pub(super) fn rename_noreplace(from: &Path, to: &Path) -> Result<(), String> {
    rename_noreplace_io(from, to).map_err(|e| format!("cannot rename {}: {e}", from.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    fn sandbox() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "ns-transfer-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }
    #[test]
    fn refuses_existing_destination_without_touching_files() {
        let root = sandbox();
        let source = root.join("source");
        let target = root.join("target");
        std::fs::write(&source, "source bytes").unwrap();
        std::fs::write(&target, "keep these bytes").unwrap();
        assert!(copy_path_noreplace(&source, &target).is_err());
        assert_eq!(std::fs::read(&source).unwrap(), b"source bytes");
        assert_eq!(std::fs::read(&target).unwrap(), b"keep these bytes");
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn transfer_copies_files_folders_and_batch_without_overwrite() {
        let root = sandbox();
        let src = root.join("src");
        let dst = root.join("dst");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        let file = src.join("note.txt");
        std::fs::write(&file, "original").unwrap();
        let folder = src.join("bundle");
        std::fs::create_dir_all(folder.join("nested")).unwrap();
        std::fs::write(folder.join("nested/file.txt"), "deep").unwrap();
        std::fs::write(dst.join("note.txt"), "keep").unwrap();
        let batch = vec![file.clone(), folder.clone()];
        assert!(transfer(&batch, &dst, false).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), b"original");
        assert_eq!(std::fs::read(dst.join("note.txt")).unwrap(), b"keep");
        assert!(!dst.join("bundle").exists());
        std::fs::remove_file(dst.join("note.txt")).unwrap();
        transfer(&batch, &dst, false).unwrap();
        assert_eq!(std::fs::read(dst.join("note.txt")).unwrap(), b"original");
        assert_eq!(
            std::fs::read(dst.join("bundle/nested/file.txt")).unwrap(),
            b"deep"
        );
        assert!(file.exists() && folder.exists());
        assert!(transfer(std::slice::from_ref(&file), &dst, true).is_err());
        assert!(file.exists());
        let moved = src.join("solo.txt");
        std::fs::write(&moved, "solo").unwrap();
        transfer(std::slice::from_ref(&moved), &dst, true).unwrap();
        assert!(!moved.exists());
        assert_eq!(std::fs::read(dst.join("solo.txt")).unwrap(), b"solo");
        std::fs::remove_dir_all(root).unwrap();
    }
}
