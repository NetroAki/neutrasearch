//! Turns one Btrfs sweep into index changes. For every directory whose entry
//! list changed it compares the names the filesystem has now with the names
//! the index holds, so deletes, renames and new folders surface without ever
//! reading a directory. Changed inodes then refresh sizes and times.

use anyhow::Result;
use neutra_btrfs::{Changes, Child, Inode};
use neutra_core::{CompactIndex, DeltaChange, DeltaIndex, FileKind, FileRecord, FsKind};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

/// Bounded expansion for directory deletes and renames. Larger trees need a
/// prefix tombstone in the delta API to avoid materializing every record.
#[cfg(not(test))]
const SUBTREE_LIMIT: usize = 2_000_000;
#[cfg(test)]
const SUBTREE_LIMIT: usize = 3;

/// The filesystem side of a reconcile, so the logic runs against a fake in tests.
pub(crate) trait Tree {
    fn dir_path(&self, ino: u64) -> Result<Option<String>>;
    fn children(&self, dir: u64) -> Result<Vec<Child>>;
    fn inode(&self, ino: u64) -> Result<Option<Inode>>;
    fn parent_refs(&self, ino: u64) -> Result<Vec<(u64, String)>>;
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
    fn parent_refs(&self, ino: u64) -> Result<Vec<(u64, String)>> {
        neutra_btrfs::Subvolume::parent_refs(self, ino)
    }
}

/// Where a subvolume is mounted and how its records are labelled.
pub(crate) struct Mounted<'a> {
    /// Mount point without a trailing slash ("" for "/").
    pub prefix: &'a str,
    /// Btrfs path that is exposed as the mount root, e.g. `/@/home`.
    pub relative_root: &'a str,
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

fn mount_relative(root: &str, relative: &str) -> Option<String> {
    let root = root.trim_matches('/');
    let rel = relative.trim_matches('/');
    if root.is_empty() {
        return Some(rel.to_owned());
    }
    if rel == root {
        return Some(String::new());
    }
    rel.strip_prefix(root)?.strip_prefix('/').map(str::to_owned)
}

fn child_path(dir: &str, name: &str) -> String {
    if dir == "/" {
        format!("/{name}")
    } else {
        format!("{dir}/{name}")
    }
}

fn record(path: String, inode: &Inode, parent: u64, mounted: &Mounted) -> FileRecord {
    FileRecord {
        path: path.into_boxed_str(),
        size: inode.size,
        disk: FileRecord::allocated_bytes(inode.disk),
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

fn indexed(
    base: &CompactIndex,
    delta: Option<&DeltaIndex>,
    browser: Option<&neutra_core::BrowserIndex>,
    path: &str,
) -> Result<Option<FileRecord>> {
    if let Some(delta) = delta {
        if let Some(found) = delta.upsert_for(path)? {
            return Ok(Some(found));
        }
        if delta.is_removed(path)? {
            return Ok(None);
        }
    }
    if let Some(browser) = browser {
        return Ok(browser.record_by_path(path)?);
    }
    Ok(base.record_by_path_any_source(path)?)
}

fn inode_of(tree: &dyn Tree, known: &mut HashMap<u64, &Inode>, ino: u64) -> Result<Option<Inode>> {
    match known.remove(&ino) {
        Some(inode) => Ok(Some((*inode).clone())),
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
    browser: Option<&neutra_core::BrowserIndex>,
) -> Result<Vec<DeltaChange>> {
    let mut out = BTreeMap::<String, DeltaChange>::new();
    let mut inodes: HashMap<u64, &Inode> = changes
        .inodes
        .iter()
        .map(|(inode, _)| (inode.ino, inode))
        .collect();
    let mut removed = Vec::<FileRecord>::new();
    let mut added = Vec::<(String, u64, Child)>::new();
    // A folder whose last entry was removed has no entry items left to show it
    // changed, but its own inode (size, mtime) did, so changed folders count too.
    let touched = changes.dirs.iter().copied().chain(
        changes
            .inodes
            .iter()
            .filter(|(inode, _)| inode.kind == FileKind::Dir)
            .map(|(inode, _)| inode.ino),
    );
    let mut queue: VecDeque<u64> = touched.collect();
    let mut seen: HashSet<u64> = queue.iter().copied().collect();
    while let Some(dir) = queue.pop_front() {
        let Some(relative) = tree.dir_path(dir)? else {
            continue;
        };
        let Some(relative) = mount_relative(mounted.relative_root, &relative) else {
            continue;
        };
        let dir_path = full_path(mounted.prefix, &relative);
        let now = tree.children(dir)?;
        let known = match browser {
            Some(browser) => browser.directory_children(&dir_path)?,
            None => base.directory_children(&dir_path, delta)?,
        };
        let now_names: HashSet<&str> = now.iter().map(|child| child.name.as_str()).collect();
        let known_names: HashSet<&str> = known
            .iter()
            .map(|rec| rec.path.rsplit('/').next().unwrap_or(""))
            .collect();
        removed.extend(
            known
                .iter()
                .filter(|rec| !now_names.contains(rec.path.rsplit('/').next().unwrap_or("")))
                .cloned(),
        );
        for child in now
            .iter()
            .filter(|child| child.ino != 0 && !known_names.contains(child.name.as_str()))
        {
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
            let found = match browser {
                Some(browser) => browser.subtree_records(&gone.path, SUBTREE_LIMIT)?,
                None => base.subtree_records(&gone.path, delta, SUBTREE_LIMIT)?,
            };
            if let Some(found) = found {
                subtree = found;
            } else {
                anyhow::bail!("cannot reconcile folder {}: more than {SUBTREE_LIMIT} descendants require native prefix updates; the cursor was not advanced", gone.path);
            }
        }
        out.insert(
            gone.path.to_string(),
            DeltaChange::Remove(gone.path.clone()),
        );
        for below in &subtree {
            out.insert(
                below.path.to_string(),
                DeltaChange::Remove(below.path.clone()),
            );
        }
        if let Some(position) = target {
            let (dir_path, parent, child) = &added[position];
            let new_path = child_path(dir_path, &child.name);
            if let Some(inode) = inode_of(tree, &mut inodes, child.ino)? {
                out.insert(
                    new_path.clone(),
                    DeltaChange::Upsert(record(new_path.clone(), &inode, *parent, mounted)),
                );
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
            {
                if let Some(inode) = inode_of(tree, &mut inodes, child.ino)? {
                    out.insert(
                        path.clone(),
                        DeltaChange::Upsert(record(path, &inode, parent, mounted)),
                    );
                }
                if child.kind == FileKind::Dir && seen.insert(child.ino) {
                    let relative_dir = child_path(&dir_path, &child.name);
                    let now = tree.children(child.ino)?;
                    let known = match browser {
                        Some(browser) => browser.directory_children(&relative_dir)?,
                        None => base.directory_children(&relative_dir, delta)?,
                    };
                    let known_names: HashSet<&str> = known
                        .iter()
                        .map(|rec| rec.path.rsplit('/').next().unwrap_or(""))
                        .collect();
                    for below in now.into_iter().filter(|below| {
                        below.ino != 0 && !known_names.contains(below.name.as_str())
                    }) {
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
        let Some(inode) = inodes.get(&inode.ino).copied() else {
            continue;
        };
        // Read the inode's complete refs, including links in older leaves.
        let mut links = tree.parent_refs(inode.ino)?;
        if let Some(link) = link {
            links.push(link.clone());
        }
        links.sort();
        links.dedup();
        for (parent, name) in links {
            if let std::collections::hash_map::Entry::Vacant(e) = parents.entry(parent) {
                let found = tree
                    .dir_path(parent)?
                    .and_then(|relative| mount_relative(mounted.relative_root, &relative))
                    .map(|relative| full_path(mounted.prefix, &relative));
                e.insert(found);
            }
            let Some(dir_path) = parents.get(&parent).cloned().flatten() else {
                continue;
            };
            let path = child_path(&dir_path, &name);
            if out.contains_key(&path) {
                continue;
            }
            let fresh = record(path.clone(), inode, parent, mounted);
            if !indexed(base, delta, browser, &path)?
                .is_some_and(|known| same_content(&known, &fresh))
            {
                out.insert(path, DeltaChange::Upsert(fresh));
            }
        }
    }
    Ok(out.into_values().collect())
}

#[cfg(test)]
mod tests;
