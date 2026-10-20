//! Turns one Btrfs sweep into index changes. For every directory whose entry
//! list changed it compares the names the filesystem has now with the names
//! the index holds, so deletes, renames and new folders surface without ever
//! reading a directory. Changed inodes then refresh sizes and times.

use anyhow::Result;
use neutra_btrfs::{Changes, Child, Inode};
use neutra_core::{CompactIndex, DeltaChange, DeltaIndex, FileKind, FileRecord, FsKind};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

mod plan;

/// Directory moves still expand descendants; larger moves need durable prefix updates.
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
    let mut plan = plan::Plan::new(tree, changes, base, delta, mounted, browser);
    let removed = plan.discover(changes)?;
    plan.moves(removed)?;
    plan.additions()?;
    plan.refresh(changes)?;
    Ok(plan.finish())
}

#[cfg(test)]
mod tests;
