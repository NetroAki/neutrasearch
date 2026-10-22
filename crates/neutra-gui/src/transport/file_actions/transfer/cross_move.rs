//! Source cleanup for a move that crossed filesystems. The copy takes a while,
//! so the source is only removed where it still matches what existed before
//! the copy began. Anything created or edited meanwhile stays in place.

use std::{
    fs, io,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

pub(super) struct Snapshot(Vec<(PathBuf, Key)>);

struct Key {
    dir: bool,
    ino: u64,
    size: u64,
    mtime: (i64, i64),
    // ctime cannot be set from user space, so it exposes an edit that restored
    // the old modification time.
    ctime: (i64, i64),
    linked: bool,
}

/// Flush the whole filesystem that holds `path` to disk.
pub(super) fn sync_volume(path: &Path) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    // The parent is a real directory on the same volume even when the entry is a link.
    let handle = fs::File::open(path.parent().unwrap_or(Path::new(".")))?;
    // SAFETY: the descriptor stays open for the duration of the call.
    if unsafe { libc::syncfs(handle.as_raw_fd()) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn key(path: &Path) -> io::Result<Key> {
    let m = fs::symlink_metadata(path)?;
    let dir = m.is_dir();
    Ok(Key {
        dir,
        ino: m.ino(),
        // Folder size and time change as children go, so only identity counts.
        size: if dir { 0 } else { m.size() },
        mtime: if dir {
            (0, 0)
        } else {
            (m.mtime(), m.mtime_nsec())
        },
        ctime: if dir {
            (0, 0)
        } else {
            (m.ctime(), m.ctime_nsec())
        },
        // Removing one link changes the ctime of the others, so those files rely
        // on identity, size and mtime.
        linked: m.nlink() > 1,
    })
}

pub(super) struct Outcome {
    /// Entries that changed, or folders that still hold something.
    pub(super) left: usize,
    /// Folder holding a file that was replaced while the cleanup ran.
    pub(super) kept_at: Option<PathBuf>,
}

impl Key {
    fn matches(&self, now: &Key) -> bool {
        self.same_content(now) && (self.linked || self.ctime == now.ctime)
    }

    /// What must still hold once a file was renamed aside, which updates its ctime.
    fn same_content(&self, other: &Key) -> bool {
        self.dir == other.dir
            && self.ino == other.ino
            && self.size == other.size
            && self.mtime == other.mtime
    }
}

impl Snapshot {
    pub(super) fn take(root: &Path) -> io::Result<Self> {
        let mut entries = Vec::new();
        collect(root, &mut entries)?;
        Ok(Self(entries))
    }

    /// Remove what still matches. Each file is first renamed into a private
    /// holding folder, which cannot replace anything, and is deleted only if the
    /// moved entry is the one that was copied; a file saved over the path at that
    /// moment is renamed back instead of being lost. Folders go last, children
    /// first, with remove_dir so any folder that holds something stays.
    pub(super) fn remove_unchanged(self, root: &Path) -> Outcome {
        let mut left = 0;
        let mut kept_at = None;
        let hold = hold_folder(root);
        let mut made = false;
        let (folders, files): (Vec<_>, Vec<_>) = self.0.into_iter().partition(|(_, k)| k.dir);
        for (n, (path, before)) in files.into_iter().enumerate() {
            match key(&path) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Ok(now) if before.matches(&now) => {}
                _ => {
                    left += 1;
                    continue;
                }
            }
            if !made {
                made = fs::create_dir(&hold).is_ok();
            }
            let held = hold.join(n.to_string());
            if !made || super::rename_noreplace_io(&path, &held).is_err() {
                left += 1;
                continue;
            }
            match key(&held) {
                Ok(now) if now.same_content(&before) => {
                    if fs::remove_file(&held).is_err() {
                        left += 1;
                        kept_at = Some(hold.clone());
                    }
                }
                _ => {
                    left += 1;
                    if super::rename_noreplace_io(&held, &path).is_err() {
                        kept_at = Some(hold.clone());
                    }
                }
            }
        }
        if made {
            let _ = fs::remove_dir(&hold);
        }
        for (path, before) in folders.into_iter().rev() {
            match key(&path) {
                Ok(now) if before.matches(&now) => {
                    if fs::remove_dir(&path).is_err() {
                        left += 1;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                _ => left += 1,
            }
        }
        Outcome { left, kept_at }
    }
}

fn hold_folder(root: &Path) -> PathBuf {
    let parent = if root.is_dir() {
        root
    } else {
        root.parent().unwrap_or(Path::new("."))
    };
    parent.join(format!(".neutrasearch-hold-{}", std::process::id()))
}

fn collect(path: &Path, out: &mut Vec<(PathBuf, Key)>) -> io::Result<()> {
    let current = key(path)?;
    let dir = current.dir;
    out.push((path.to_path_buf(), current));
    if dir {
        for entry in fs::read_dir(path)? {
            collect(&entry?.path(), out)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_a_same_size_edit_that_restored_its_modification_time() {
        let root = std::env::temp_dir().join(format!("ns-cross-ctime-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let file = root.join("f");
        fs::write(&file, "aaaa").unwrap();
        let modified = fs::metadata(&file).unwrap().modified().unwrap();
        let snapshot = Snapshot::take(&root).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(&file, "bbbb").unwrap();
        fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        assert_eq!(snapshot.remove_unchanged(&root).left, 2);
        assert_eq!(fs::read(&file).unwrap(), b"bbbb");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn keeps_files_that_appear_or_change_after_the_snapshot() {
        let root = std::env::temp_dir().join(format!("ns-cross-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("a/b")).unwrap();
        fs::write(root.join("a/same"), "x").unwrap();
        fs::write(root.join("a/b/edited"), "one").unwrap();
        let snapshot = Snapshot::take(&root.join("a")).unwrap();
        fs::write(root.join("a/new"), "late").unwrap();
        fs::write(root.join("a/b/edited"), "two, longer").unwrap();
        assert_eq!(snapshot.remove_unchanged(&root.join("a")).left, 3);
        assert!(!root.join("a/same").exists());
        assert_eq!(fs::read(root.join("a/new")).unwrap(), b"late");
        assert_eq!(fs::read(root.join("a/b/edited")).unwrap(), b"two, longer");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn removes_a_tree_with_internal_hard_links() {
        let root = std::env::temp_dir().join(format!("ns-cross-links-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("tree")).unwrap();
        fs::write(root.join("tree/one"), "shared").unwrap();
        fs::hard_link(root.join("tree/one"), root.join("tree/two")).unwrap();
        let snapshot = Snapshot::take(&root.join("tree")).unwrap();
        let outcome = snapshot.remove_unchanged(&root.join("tree"));
        assert_eq!(outcome.left, 0);
        assert!(!root.join("tree").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn keeps_a_file_replaced_by_a_new_inode_after_the_snapshot() {
        let root = std::env::temp_dir().join(format!("ns-cross-swap-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("f"), "old").unwrap();
        let snapshot = Snapshot::take(&root).unwrap();
        fs::write(root.join("tmp"), "new!").unwrap();
        fs::rename(root.join("tmp"), root.join("f")).unwrap();
        let outcome = snapshot.remove_unchanged(&root);
        assert_eq!(outcome.left, 2);
        assert_eq!(fs::read(root.join("f")).unwrap(), b"new!");
        fs::remove_dir_all(root).unwrap();
    }
}
