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
    dev: u64,
    ino: u64,
    size: u64,
    mtime: (i64, i64),
    // ctime cannot be set from user space, so it exposes an edit that restored
    // the old modification time.
    ctime: (i64, i64),
    linked: bool,
    digest: Option<[u8; 32]>,
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

fn key(path: &Path, force_digest: bool) -> io::Result<Key> {
    let m = fs::symlink_metadata(path)?;
    let dir = m.is_dir();
    Ok(Key {
        dir,
        dev: m.dev(),
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
        // Removing one link changes the ctime of the others, so regular files
        // rely on identity, size, mtime and a digest of their contents.
        linked: m.nlink() > 1,
        digest: if !dir && (force_digest || m.nlink() > 1) && m.file_type().is_file() {
            Some(hardlink_digest::read(path, &m)?)
        } else {
            None
        },
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
            && self.dev == other.dev
            && self.ino == other.ino
            && self.size == other.size
            && self.mtime == other.mtime
            && self.digest == other.digest
    }

    fn still_at(&self, path: &Path) -> bool {
        fs::symlink_metadata(path).is_ok_and(|m| {
            m.dev() == self.dev
                && m.ino() == self.ino
                && m.size() == self.size
                && (m.mtime(), m.mtime_nsec()) == self.mtime
                && (m.ctime(), m.ctime_nsec()) == self.ctime
        })
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
    pub(super) fn remove_unchanged(self, root: &Path, destination: &Path) -> Outcome {
        self.remove_checked(root, destination, hardlink_digest::matches)
    }

    fn remove_checked(
        self,
        root: &Path,
        destination: &Path,
        mut copied_matches: impl FnMut(&Path, &[u8; 32]) -> bool,
    ) -> Outcome {
        let mut left = 0;
        let mut kept_at = None;
        let hold = hold_folder(root);
        let mut made = false;
        let (folders, files): (Vec<_>, Vec<_>) = self.0.into_iter().partition(|(_, k)| k.dir);
        for (n, (path, before)) in files.into_iter().enumerate() {
            let copied = path.strip_prefix(root).ok().map(|relative| {
                if relative.as_os_str().is_empty() {
                    destination.to_path_buf()
                } else {
                    destination.join(relative)
                }
            });
            match key(&path, before.digest.is_some()) {
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
            match key(&held, before.digest.is_some()) {
                // The hold rename changes ctime, so digest hardlinked bytes again.
                Ok(now)
                    if now.same_content(&before)
                        && before.digest.as_ref().is_none_or(|digest| {
                            copied
                                .as_ref()
                                .is_some_and(|path| copied_matches(path, digest))
                        })
                        && now.still_at(&held) =>
                {
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
            match key(&path, false) {
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
    let current = key(path, false)?;
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
#[path = "cross_move/tests.rs"]
mod tests;

#[path = "cross_move/hardlink_digest.rs"]
mod hardlink_digest;
