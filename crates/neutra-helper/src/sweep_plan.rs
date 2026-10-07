//! Turns one Btrfs sweep into index changes. For every directory whose entry
//! list changed it compares the names the filesystem has now with the names
//! the index holds, so deletes, renames and new folders surface without ever
//! reading a directory. Changed inodes then refresh sizes and times.

use crate::watch_mount::NOISY_DIRS;
use anyhow::Result;
use neutra_btrfs::{Changes, Child, Inode};
use neutra_core::{CompactIndex, DeltaChange, DeltaIndex, FileKind, FileRecord, FsKind};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

/// A removed directory larger than this is dropped from the index by name
/// only; its contents wait for the next rebuild.
const SUBTREE_LIMIT: usize = 2_000_000;

/// Directories that change in most sweeps are build output or logs. After a
/// few busy sweeps in a row they are skipped until they settle, which bounds
/// how much a runaway writer can add to the delta log.
#[derive(Default)]
pub(crate) struct Churn {
    recent: HashMap<u64, u8>,
}

impl Churn {
    const BUSY_SWEEPS: u32 = 4;

    /// Age every directory's history by one sweep.
    pub(crate) fn begin_sweep(&mut self) {
        self.recent.retain(|_, history| {
            *history <<= 1;
            *history != 0
        });
    }

    /// Record a change in `dir`; false while it is too busy to follow.
    fn allow(&mut self, dir: u64) -> bool {
        let history = self.recent.entry(dir).or_insert(0);
        *history |= 1;
        history.count_ones() <= Self::BUSY_SWEEPS
    }
}

/// The filesystem side of a reconcile, so the logic runs against a fake in tests.
pub(crate) trait Tree {
    fn dir_path(&self, ino: u64) -> Result<Option<String>>;
    fn children(&self, dir: u64) -> Result<Vec<Child>>;
    fn inode(&self, ino: u64) -> Result<Option<Inode>>;
    fn parent_ref(&self, ino: u64) -> Result<Option<(u64, String)>>;
}

impl Tree for neutra_btrfs::Subvolume {
    fn dir_path(&self, ino: u64) -> Result<Option<String>> {
        neutra_btrfs::Subvolume::dir_path(self, ino)
    }
    fn children(&self, dir: u64) -> Result<Vec<Child>> {
        neutra_btrfs::Subvolume::children(self, dir)
    }
    fn inode(&self, ino: u64) -> Result<Option<Inode>> {
        neutra_btrfs::Subvolume::inode(self, ino)
    }
    fn parent_ref(&self, ino: u64) -> Result<Option<(u64, String)>> {
        neutra_btrfs::Subvolume::parent_ref(self, ino)
    }
}

/// Where a subvolume is mounted and how its records are labelled.
pub(crate) struct Mounted<'a> {
    /// Mount point without a trailing slash ("" for "/").
    pub prefix: &'a str,
    pub fs: &'a FsKind,
    pub source: u32,
}

fn full_path(prefix: &str, relative: &str) -> String {
    match (prefix.is_empty(), relative.is_empty()) {
        (true, true) => "/".into(),
        (false, true) => prefix.into(),
        _ => format!("{prefix}/{relative}"),
    }
}

fn child_path(dir: &str, name: &str) -> String {
    if dir == "/" { format!("/{name}") } else { format!("{dir}/{name}") }
}

fn noisy(path: &str) -> bool {
    path.split('/').any(|part| NOISY_DIRS.contains(&part))
}

fn record(path: String, inode: &Inode, parent: u64, mounted: &Mounted) -> FileRecord {
    FileRecord {
        path: path.into_boxed_str(),
        size: inode.size,
        disk: inode.disk,
        mtime: inode.mtime,
        mode: inode.mode,
        kind: inode.kind,
        fs: mounted.fs.clone(),
        native_id: inode.ino,
        native_parent: parent,
        source: mounted.source,
    }
}

fn same_content(left: &FileRecord, right: &FileRecord) -> bool {
    (left.size, left.disk, left.mtime, left.mode, left.kind)
        == (right.size, right.disk, right.mtime, right.mode, right.kind)
}

fn indexed(base: &CompactIndex, delta: Option<&DeltaIndex>, path: &str) -> Option<FileRecord> {
    if let Some(delta) = delta {
        if let Some(found) = delta.upsert_for(path) {
            return Some(found.clone());
        }
        if delta.is_removed(path) {
            return None;
        }
    }
    base.record_by_path_any_source(path).ok().flatten()
}

fn inode_of(tree: &dyn Tree, known: &mut HashMap<u64, Inode>, ino: u64) -> Result<Option<Inode>> {
    match known.remove(&ino) {
        Some(inode) => Ok(Some(inode)),
        None => tree.inode(ino),
    }
}

/// Everything the index must change to match the filesystem after `changes`.
pub(crate) fn reconcile(
    tree: &dyn Tree,
    changes: &Changes,
    base: &CompactIndex,
    delta: Option<&DeltaIndex>,
    mounted: &Mounted,
    churn: &mut Churn,
) -> Result<Vec<DeltaChange>> {
    let mut out = BTreeMap::<String, DeltaChange>::new();
    let mut inodes: HashMap<u64, Inode> = changes.inodes.iter().map(|(inode, _)| (inode.ino, inode.clone())).collect();
    let mut removed = Vec::<FileRecord>::new();
    let mut added = Vec::<(String, u64, Child)>::new();
    // A folder whose last entry was removed has no entry items left to show it
    // changed, but its own inode (size, mtime) did, so changed folders count too.
    let touched = changes.dirs.iter().copied().chain(
        changes.inodes.iter().filter(|(inode, _)| inode.kind == FileKind::Dir).map(|(inode, _)| inode.ino),
    );
    let mut queue: VecDeque<u64> = touched.collect();
    let mut seen: HashSet<u64> = queue.iter().copied().collect();
    while let Some(dir) = queue.pop_front() {
        if !churn.allow(dir) {
            continue;
        }
        let Some(relative) = tree.dir_path(dir)? else { continue };
        let dir_path = full_path(mounted.prefix, &relative);
        if noisy(&dir_path) {
            continue;
        }
        let now = tree.children(dir)?;
        let known = base.directory_children(&dir_path, delta)?;
        let now_names: HashSet<&str> = now.iter().map(|child| child.name.as_str()).collect();
        let known_names: HashSet<&str> = known.iter().map(|rec| rec.path.rsplit('/').next().unwrap_or("")).collect();
        removed.extend(known.iter().filter(|rec| !now_names.contains(rec.path.rsplit('/').next().unwrap_or(""))).cloned());
        for child in now.iter().filter(|child| child.ino != 0 && !known_names.contains(child.name.as_str())) {
            added.push((dir_path.clone(), dir, child.clone()));
        }
    }
    // A name that left one directory and appeared in another with the same
    // inode is a move: carry its records over instead of rebuilding them.
    let mut moved = HashMap::<u64, usize>::new();
    for (position, (_, _, child)) in added.iter().enumerate() {
        moved.insert(child.ino, position);
    }
    let mut handled = HashSet::<usize>::new();
    for gone in &removed {
        let target = (gone.fs == *mounted.fs && gone.source == mounted.source)
            .then(|| moved.get(&gone.native_id).copied())
            .flatten()
            .filter(|position| added[*position].2.kind == gone.kind);
        let mut subtree = Vec::new();
        if gone.kind == FileKind::Dir {
            match base.subtree_records(&gone.path, delta, SUBTREE_LIMIT)? {
                Some(found) => subtree = found,
                None => tracing::warn!(path = %gone.path, "removed folder too large; its contents wait for the next rebuild"),
            }
        }
        out.insert(gone.path.to_string(), DeltaChange::Remove(gone.path.clone()));
        for below in &subtree {
            out.insert(below.path.to_string(), DeltaChange::Remove(below.path.clone()));
        }
        if let Some(position) = target {
            let (dir_path, parent, child) = &added[position];
            let new_path = child_path(dir_path, &child.name);
            if let Some(inode) = inode_of(tree, &mut inodes, child.ino)? {
                out.insert(new_path.clone(), DeltaChange::Upsert(record(new_path.clone(), &inode, *parent, mounted)));
            }
            for below in subtree {
                let moved_path = format!("{new_path}{}", &below.path[gone.path.len()..]);
                let mut copy = below;
                copy.path = moved_path.clone().into_boxed_str();
                out.insert(moved_path, DeltaChange::Upsert(copy));
            }
            handled.insert(position);
        }
    }
    // New names: record them, and descend into new folders (their contents
    // sit in leaves this sweep may not have listed as changed directories).
    let mut position = 0;
    while position < added.len() {
        if !handled.contains(&position) {
            let (dir_path, parent, child) = added[position].clone();
            let path = child_path(&dir_path, &child.name);
            if !noisy(&path) {
                if let Some(inode) = inode_of(tree, &mut inodes, child.ino)? {
                    out.insert(path.clone(), DeltaChange::Upsert(record(path, &inode, parent, mounted)));
                }
                if child.kind == FileKind::Dir && seen.insert(child.ino) {
                    let relative_dir = child_path(&dir_path, &child.name);
                    let now = tree.children(child.ino)?;
                    let known = base.directory_children(&relative_dir, delta)?;
                    let known_names: HashSet<&str> = known.iter().map(|rec| rec.path.rsplit('/').next().unwrap_or("")).collect();
                    for below in now.into_iter().filter(|below| below.ino != 0 && !known_names.contains(below.name.as_str())) {
                        added.push((relative_dir.clone(), child.ino, below));
                    }
                }
            }
        }
        position += 1;
    }
    // Changed inodes: refresh size, times and mode where the index disagrees.
    let mut parents = HashMap::<u64, Option<String>>::new();
    for (inode, link) in &changes.inodes {
        let Some(inode) = inodes.get(&inode.ino).cloned() else { continue };
        let link = match link.clone() {
            Some(link) => Some(link),
            None => tree.parent_ref(inode.ino)?,
        };
        let Some((parent, name)) = link else { continue };
        if !churn.allow(parent) {
            continue;
        }
        if !parents.contains_key(&parent) {
            let found = tree.dir_path(parent)?.map(|relative| full_path(mounted.prefix, &relative));
            parents.insert(parent, found);
        }
        let Some(dir_path) = parents.get(&parent).cloned().flatten() else { continue };
        let path = child_path(&dir_path, &name);
        if noisy(&path) || out.contains_key(&path) {
            continue;
        }
        let fresh = record(path.clone(), &inode, parent, mounted);
        if !indexed(base, delta, &path).is_some_and(|known| same_content(&known, &fresh)) {
            out.insert(path, DeltaChange::Upsert(fresh));
        }
    }
    Ok(out.into_values().collect())
}

#[cfg(test)]
mod tests;
