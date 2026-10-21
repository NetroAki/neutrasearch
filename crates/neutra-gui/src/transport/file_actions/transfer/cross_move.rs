//! Source cleanup for a move that crossed filesystems. The copy takes a while,
//! so the source is only removed where it still matches what existed before
//! the copy began. Anything created or edited meanwhile stays in place.

use std::{
    fs, io,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

pub(super) struct Snapshot(Vec<(PathBuf, Key)>);

#[derive(PartialEq)]
struct Key {
    dir: bool,
    ino: u64,
    size: u64,
    mtime: (i64, i64),
    // ctime cannot be set from user space, so it exposes an edit that restored
    // the old modification time.
    ctime: (i64, i64),
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
    })
}

impl Snapshot {
    pub(super) fn take(root: &Path) -> io::Result<Self> {
        let mut entries = Vec::new();
        collect(root, &mut entries)?;
        Ok(Self(entries))
    }

    /// Remove what still matches, children before parents. Returns how many
    /// entries were left because they changed or are no longer empty.
    pub(super) fn remove_unchanged(self) -> usize {
        let mut left = 0;
        for (path, before) in self.0.into_iter().rev() {
            let removed = match key(&path) {
                Ok(now) if now == before => {
                    if before.dir {
                        fs::remove_dir(&path)
                    } else {
                        fs::remove_file(&path)
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                _ => Err(io::Error::other("changed")),
            };
            if removed.is_err() {
                left += 1;
            }
        }
        left
    }
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
    fn keeps_files_that_appear_or_change_after_the_snapshot() {
        let root = std::env::temp_dir().join(format!("ns-cross-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("a/b")).unwrap();
        fs::write(root.join("a/same"), "x").unwrap();
        fs::write(root.join("a/b/edited"), "one").unwrap();
        let snapshot = Snapshot::take(&root.join("a")).unwrap();
        fs::write(root.join("a/new"), "late").unwrap();
        fs::write(root.join("a/b/edited"), "two, longer").unwrap();
        assert_eq!(snapshot.remove_unchanged(), 3);
        assert!(!root.join("a/same").exists());
        assert_eq!(fs::read(root.join("a/new")).unwrap(), b"late");
        assert_eq!(fs::read(root.join("a/b/edited")).unwrap(), b"two, longer");
        fs::remove_dir_all(root).unwrap();
    }
}
