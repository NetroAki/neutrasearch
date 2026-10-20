//! Lazy tree model under a fixed byte budget. Views only render the current
//! folder, its ancestors, and expanded branches, so the rest is fetched on
//! demand and evicted when stale.

use super::*;
use neutra_core::DirectorySummaryEntry;
use std::collections::BTreeSet;

#[derive(Clone)]
pub(crate) struct TreeFile {
    pub(crate) path: String,
    pub(crate) size: u64,
}

#[derive(Clone)]
pub(crate) struct DirChild {
    pub(crate) path: String,
    pub(crate) size: u64,
    pub(crate) count: u64,
}

#[derive(Clone, Default)]
pub(crate) struct FolderSummary {
    pub(crate) size: u64,
    pub(crate) count: u64,
    pub(crate) children: Vec<DirChild>,
    pub(crate) direct_files: Vec<TreeFile>,
    pub(crate) files_truncated: bool,
    /// False when file rows were stripped; navigating here refetches them.
    pub(crate) files_complete: bool,
}

pub(crate) struct Hierarchy {
    pub(crate) folders: std::collections::HashMap<String, FolderSummary>,
    bytes: u64,
    order: std::collections::VecDeque<String>,
}

impl Hierarchy {
    pub(crate) const MAX_FILE_ROWS: usize = 200;
    /// Retained listings stay under this budget.
    pub(crate) const CACHE_BYTES: u64 = 8 * 1024 * 1024;

    pub(crate) fn empty() -> Self {
        Self {
            folders: std::collections::HashMap::new(),
            bytes: 0,
            order: std::collections::VecDeque::new(),
        }
    }

    fn estimate(dir: &str, folder: &FolderSummary) -> u64 {
        dir.len() as u64
            + folder
                .direct_files
                .iter()
                .map(|file| file.path.len() as u64 + 120)
                .sum::<u64>()
            + folder
                .children
                .iter()
                .map(|child| child.path.len() as u64 + 100)
                .sum::<u64>()
    }

    /// Root, the current chain, and everything expanded: the fetch set.
    pub(crate) fn want_dirs(current: &str, expanded: &BTreeSet<String>) -> Vec<String> {
        let mut want = vec!["/".to_owned()];
        want.extend(ancestor_paths(current));
        if !want.iter().any(|path| path == current) {
            want.push(current.to_owned());
        }
        want.extend(expanded.iter().cloned());
        want.sort();
        want.dedup();
        want
    }

    /// Wanted directories missing from the model. A cached folder without
    /// file rows only satisfies background branches, never the current one.
    pub(crate) fn missing_dirs(
        model: Option<&Hierarchy>,
        current: &str,
        expanded: &BTreeSet<String>,
    ) -> Vec<String> {
        Self::want_dirs(current, expanded)
            .into_iter()
            .filter(|path| match model.and_then(|h| h.folders.get(path)) {
                None => true,
                Some(folder) => path == current && !folder.files_complete,
            })
            .collect()
    }

    /// Build one folder from a fetched listing, joining relative names.
    pub(crate) fn folder_from_listing(
        dir: &str,
        listing: neutra_core::DirListing,
    ) -> FolderSummary {
        let join = |name: &str| neutra_core::join_child_path(dir, name);
        FolderSummary {
            size: listing.total_size,
            count: listing.total_count,
            children: listing
                .subdirs
                .iter()
                .map(|child| DirChild {
                    path: join(child.name.as_ref()),
                    size: child.size,
                    count: child.files,
                })
                .collect(),
            direct_files: listing
                .files
                .iter()
                .map(|file| TreeFile {
                    path: join(file.name.as_ref()),
                    size: file.size,
                })
                .collect(),
            files_truncated: listing.files_truncated,
            files_complete: true,
        }
    }

    /// Insert one folder, keeping file rows only where asked.
    pub(crate) fn insert(&mut self, dir: String, mut folder: FolderSummary, keep_files: bool) {
        if !keep_files {
            folder.direct_files.clear();
            folder.files_complete = false;
        } else {
            folder.files_complete = true;
        }
        if let Some(old) = self.folders.insert(dir.clone(), folder) {
            self.bytes = self.bytes.saturating_sub(Self::estimate(&dir, &old));
            self.order.retain(|path| path != &dir);
        }
        let added = Self::estimate(&dir, &self.folders[&dir]);
        self.bytes = self.bytes.saturating_add(added);
        self.order.push_back(dir);
    }

    /// Merge fetched folders, then evict whatever the visible set drops.
    /// Eviction never touches pinned paths, so rendered folders resolve.
    pub(crate) fn merge(&mut self, fetched: Hierarchy, current: &str, pinned: &[String]) {
        for (dir, folder) in fetched.folders {
            self.insert(dir.clone(), folder, dir == current);
        }
        self.evict(pinned);
    }

    fn evict(&mut self, pinned: &[String]) {
        while self.bytes > Self::CACHE_BYTES {
            let Some(oldest) = self
                .order
                .iter()
                .find(|path| !pinned.contains(path))
                .cloned()
            else {
                break;
            };
            if let Some(removed) = self.folders.remove(&oldest) {
                self.bytes = self.bytes.saturating_sub(Self::estimate(&oldest, &removed));
            }
            self.order.retain(|path| path != &oldest);
        }
    }

    /// Feed one summary entry, moving its strings. Only the legacy resident
    /// path still builds this way.
    pub(crate) fn extend(&mut self, entry: DirectorySummaryEntry) {
        let mut children = Vec::new();
        let mut direct_files = Vec::new();
        for child in entry.children {
            let size = if child.physical_bytes == 0 {
                child.logical_bytes
            } else {
                child.physical_bytes
            };
            let path = child.path.into_string();
            if child.kind == FileKind::Dir {
                children.push(DirChild {
                    path,
                    size,
                    count: 0,
                });
            } else {
                direct_files.push(TreeFile { path, size });
            }
        }
        let size = if entry.physical_bytes == 0 {
            entry.logical_bytes
        } else {
            entry.physical_bytes
        };
        let folder = FolderSummary {
            size,
            count: entry.file_count,
            children,
            direct_files,
            files_truncated: false,
            files_complete: true,
        };
        self.insert(entry.path.into_string(), folder, true);
    }

    /// Build from records through the shared summary aggregation: one sorted
    /// pass, no per-record ancestor work.
    pub(crate) fn from_records(records: &[FileRecord]) -> Self {
        let mut hierarchy = Self::empty();
        if let Ok(entries) = neutra_core::aggregate_records(records) {
            for entry in entries {
                hierarchy.extend(entry);
            }
        }
        hierarchy.folders.entry("/".into()).or_default();
        hierarchy
    }
}

#[cfg(test)]
mod tests;
