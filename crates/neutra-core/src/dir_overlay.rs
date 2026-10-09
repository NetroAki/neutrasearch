//! The live directory-summary projection: base sidecar + WAL overlay with
//! adjusted totals, direct children, and full adjusted projections. Split
//! from the sidecar format so reading/writing stays separate from deltas.

use crate::dir_summary::{
    ancestor_paths, canonical_path, compare_paths, entry_key, invalid, normalize_path, parent_path,
    ChildKindRank, DirectoryChild, DirectorySummary, DirectorySummaryEntry, Key,
};
use crate::{CompactIndex, DeltaChange, DeltaIndex, FileKind, FileRecord};
use std::collections::HashMap;
use std::io;

#[derive(Debug, Clone, Copy, Default)]
struct SummaryDelta {
    logical_bytes: i128,
    physical_bytes: i128,
    file_count: i128,
    directory_count: i128,
}

/// An in-memory delta projection for live filesystem changes. It never writes
/// the sidecar; compaction publishes a fresh generation-bound projection.
pub struct DirectorySummaryOverlay {
    base: DirectorySummary,
    adjustments: HashMap<Key, SummaryDelta>,
    child_changes: HashMap<Key, HashMap<String, Option<DirectoryChild>>>,
}

impl DirectorySummaryOverlay {
    pub fn from_delta(
        base: DirectorySummary,
        compact: &CompactIndex,
        delta: &DeltaIndex,
    ) -> io::Result<Self> {
        let mut overlay = Self {
            base,
            adjustments: HashMap::new(),
            child_changes: HashMap::new(),
        };
        delta.for_each_upsert(|record| {
            let previous = compact.records_by_path(record.path.as_ref())?;
            overlay.apply_change(&DeltaChange::Upsert(record.clone()), &previous)?;
            Ok(())
        })?;
        delta.for_each_removed(|path| {
            let previous = compact.records_by_path(&path)?;
            if !previous.is_empty() {
                overlay.apply_change(&DeltaChange::Remove(path.clone()), &previous)?;
            }
            Ok(())
        })?;
        Ok(overlay)
    }

    pub fn apply_change(
        &mut self,
        change: &DeltaChange,
        previous: &[FileRecord],
    ) -> io::Result<()> {
        match change {
            DeltaChange::Upsert(record) => {
                for previous in previous {
                    self.adjust_record(previous, -1)?;
                    self.set_child(previous, None)?;
                }
                self.adjust_record(record, 1)?;
                self.set_child(record, Some(child_for_record(record)))?;
            }
            DeltaChange::Remove(path) => {
                if previous.is_empty() {
                    return Err(invalid(format!(
                        "directory summary removal requires the previous record for {}",
                        path
                    )));
                }
                let normalized = normalize_path(path)?;
                for previous in previous {
                    let previous_path = normalize_path(previous.path.as_ref())?;
                    if canonical_path(&previous_path) != canonical_path(&normalized) {
                        return Err(invalid(
                            "directory summary removal path does not match record",
                        ));
                    }
                    self.adjust_record(previous, -1)?;
                    self.set_child(previous, None)?;
                }
            }
        }
        Ok(())
    }

    pub fn get(&self, source: u32, path: &str) -> io::Result<Option<DirectorySummaryEntry>> {
        let normalized = normalize_path(path)?;
        let Some(mut entry) = self.aggregate_entry(source, &normalized) else {
            return Ok(None);
        };
        entry.children = self.children(source, &normalized)?;
        Ok(Some(entry))
    }

    fn aggregate_entry(&self, source: u32, normalized: &str) -> Option<DirectorySummaryEntry> {
        let base = self.base.get(source, normalized).cloned();
        let delta = self
            .adjustments
            .get(&entry_key(source, normalized))
            .copied()
            .unwrap_or_default();
        if base.is_none()
            && delta.logical_bytes == 0
            && delta.physical_bytes == 0
            && delta.file_count == 0
            && delta.directory_count == 0
        {
            return None;
        }
        let mut entry = base.unwrap_or_else(|| DirectorySummaryEntry {
            source,
            path: normalized.to_owned().into_boxed_str(),
            logical_bytes: 0,
            physical_bytes: 0,
            file_count: 0,
            directory_count: 0,
            children: Vec::new(),
        });
        entry.logical_bytes = apply_unsigned_delta(entry.logical_bytes, delta.logical_bytes);
        entry.physical_bytes = apply_unsigned_delta(entry.physical_bytes, delta.physical_bytes);
        entry.file_count = apply_unsigned_delta(entry.file_count, delta.file_count);
        entry.directory_count = apply_unsigned_delta(entry.directory_count, delta.directory_count);
        Some(entry)
    }

    /// Every base entry with overlay adjustments applied. This is what turns
    /// the overlay from a per-path lookup into a consumable projection (the
    /// GUI hierarchy view builds from it while a watch helper is live).
    pub fn adjusted_entries(&self) -> io::Result<Vec<DirectorySummaryEntry>> {
        let mut out = Vec::with_capacity(self.base.entries().len());
        for entry in self.base.entries() {
            if let Some(adjusted) = self.aggregate_entry(entry.source, entry.path.as_ref()) {
                let mut adjusted = adjusted;
                adjusted.children = self.children(entry.source, entry.path.as_ref())?;
                out.push(adjusted);
            }
        }
        Ok(out)
    }

    pub fn children(&self, source: u32, path: &str) -> io::Result<Vec<DirectoryChild>> {
        let normalized = normalize_path(path)?;
        let parent_key = entry_key(source, &normalized);
        let mut children = self.base.children(source, &normalized).unwrap_or_default();
        let mut positions = children
            .iter()
            .enumerate()
            .map(|(index, child)| (canonical_path(&child.path), index))
            .collect::<HashMap<_, _>>();
        if let Some(changes) = self.child_changes.get(&parent_key) {
            for (key, change) in changes {
                match change {
                    Some(child) => {
                        if let Some(index) = positions.get(key).copied() {
                            children[index] = child.clone();
                        } else {
                            positions.insert(key.clone(), children.len());
                            children.push(child.clone());
                        }
                    }
                    None => {
                        if let Some(index) = positions.remove(key) {
                            children.swap_remove(index);
                            positions = children
                                .iter()
                                .enumerate()
                                .map(|(index, child)| (canonical_path(&child.path), index))
                                .collect();
                        }
                    }
                }
            }
        }
        for child in &mut children {
            if child.kind == FileKind::Dir {
                if let Some(entry) = self.aggregate_entry(source, &child.path) {
                    child.logical_bytes = entry.logical_bytes;
                    child.physical_bytes = entry.physical_bytes;
                    child.file_count = entry.file_count;
                    child.directory_count = entry.directory_count;
                }
            }
        }
        children.sort_unstable_by(|left, right| {
            compare_paths(&left.path, &right.path)
                .then_with(|| left.kind_rank().cmp(&right.kind_rank()))
        });
        Ok(children)
    }

    fn adjust_record(&mut self, record: &FileRecord, sign: i128) -> io::Result<()> {
        let normalized = normalize_path(record.path.as_ref())?;
        let ancestors = ancestor_paths(&normalized);
        let parent_ancestors = ancestors
            .get(..ancestors.len().saturating_sub(1))
            .unwrap_or_default();
        let contributes_file = matches!(record.kind, FileKind::File | FileKind::Symlink);
        for ancestor in parent_ancestors {
            let delta = self
                .adjustments
                .entry(entry_key(record.source, ancestor))
                .or_default();
            if contributes_file {
                delta.logical_bytes += sign * i128::from(record.size);
                delta.physical_bytes += sign * i128::from(record.disk_bytes());
                delta.file_count += sign;
            }
            if record.kind == FileKind::Dir {
                delta.directory_count += sign;
            }
        }
        if record.kind == FileKind::Dir {
            self.adjustments
                .entry(entry_key(record.source, &normalized))
                .or_default();
        }
        Ok(())
    }

    fn set_child(&mut self, record: &FileRecord, child: Option<DirectoryChild>) -> io::Result<()> {
        let normalized = normalize_path(record.path.as_ref())?;
        let parent = parent_path(&normalized);
        self.child_changes
            .entry(entry_key(record.source, &parent))
            .or_default()
            .insert(canonical_path(&normalized), child);
        Ok(())
    }
}

fn child_for_record(record: &FileRecord) -> DirectoryChild {
    DirectoryChild {
        path: normalize_path(record.path.as_ref())
            .unwrap_or_else(|_| record.path.to_string())
            .into_boxed_str(),
        kind: record.kind,
        logical_bytes: if record.kind == FileKind::Dir {
            0
        } else {
            record.size
        },
        physical_bytes: if record.kind == FileKind::Dir {
            0
        } else {
            record.disk_bytes()
        },
        file_count: u64::from(matches!(record.kind, FileKind::File | FileKind::Symlink)),
        directory_count: 0,
    }
}

fn apply_unsigned_delta(value: u64, delta: i128) -> u64 {
    let max = i128::from(u64::MAX);
    if delta >= 0 {
        value.saturating_add(delta.min(max) as u64)
    } else {
        let amount = delta.checked_neg().unwrap_or(i128::MAX).min(max) as u64;
        value.saturating_sub(amount)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dir_summary::DirectorySummary;
    use crate::{CompactIndex, DeltaChange, DeltaIndex, FileKind};

    fn record(path: &str, size: u64, kind: FileKind) -> FileRecord {
        FileRecord {
            path: path.into(),
            size,
            mtime: 0,
            mode: 0,
            kind,
            fs: crate::FsKind::Btrfs,
            native_id: size + 100,
            native_parent: 0,
            source: 0,
            disk: 0,
        }
    }

    fn cleanup(path: &std::path::Path) {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(crate::dir_summary::DirectorySummary::path_for(path));
    }

    fn remove_delta(path: &std::path::Path) {
        let mut delta_path = path.to_path_buf();
        delta_path.set_extension("delta");
        let _ = std::fs::remove_file(&delta_path);
        let mut lock = delta_path.as_os_str().to_os_string();
        lock.push(".lock");
        let _ = std::fs::remove_file(std::path::PathBuf::from(lock));
    }

    #[test]
    fn overlay_removes_all_sources_for_a_path_key() {
        let path =
            std::env::temp_dir().join(format!("neutra-dir-sources-{}.nsx", std::process::id()));
        cleanup(&path);
        let mut first = record("/shared.txt", 10, FileKind::File);
        first.source = 1;
        let mut second = record("/shared.txt", 20, FileKind::File);
        second.source = 2;
        let records = vec![first, second];
        CompactIndex::build_with_summary(&records, &path).unwrap();
        let compact = CompactIndex::open(&path).unwrap();
        let summary = DirectorySummary::open_for_compact(&path, compact.generation()).unwrap();
        let mut delta_path = path.clone();
        delta_path.set_extension("delta");
        let mut delta = DeltaIndex::open(&delta_path, compact.generation()).unwrap();
        delta
            .apply(DeltaChange::Remove("/shared.txt".into()))
            .unwrap();
        delta.sync().unwrap();
        let overlay = DirectorySummaryOverlay::from_delta(summary, &compact, &delta).unwrap();
        assert_eq!(overlay.get(1, "/").unwrap().unwrap().logical_bytes, 0);
        assert_eq!(overlay.get(2, "/").unwrap().unwrap().logical_bytes, 0);
        drop(delta);
        drop(compact);
        cleanup(&path);
        remove_delta(&path);
    }

    #[test]
    fn overlay_removal_normalizes_separator_variants() {
        let path =
            std::env::temp_dir().join(format!("neutra-dir-separators-{}.nsx", std::process::id()));
        cleanup(&path);
        let records = vec![record(r"C:\Data\report.txt", 12, FileKind::File)];
        CompactIndex::build_with_summary(&records, &path).unwrap();
        let compact = CompactIndex::open(&path).unwrap();
        assert!(compact
            .record_by_path(0, "C:/Data/report.txt")
            .unwrap()
            .is_some());
        let summary = DirectorySummary::open_for_compact(&path, compact.generation()).unwrap();
        let mut delta_path = path.clone();
        delta_path.set_extension("delta");
        let mut delta = DeltaIndex::open(&delta_path, compact.generation()).unwrap();
        delta
            .apply(DeltaChange::Remove("C:/Data/report.txt".into()))
            .unwrap();
        delta.sync().unwrap();
        let overlay = DirectorySummaryOverlay::from_delta(summary, &compact, &delta).unwrap();
        assert_eq!(overlay.get(0, "/").unwrap().unwrap().logical_bytes, 0);
        drop(delta);
        drop(compact);
        cleanup(&path);
        remove_delta(&path);
    }

    #[test]
    fn overlay_updates_ancestor_totals_and_direct_children() {
        let path =
            std::env::temp_dir().join(format!("neutra-dir-overlay-{}.nsx", std::process::id()));
        cleanup(&path);
        let records = vec![
            record("/docs", 0, FileKind::Dir),
            record("/docs/a.txt", 10, FileKind::File),
            record("/docs/sub", 0, FileKind::Dir),
            record("/docs/sub/b.txt", 20, FileKind::File),
        ];
        CompactIndex::build_with_summary(&records, &path).unwrap();
        let compact = CompactIndex::open(&path).unwrap();
        let summary = DirectorySummary::open_for_compact(&path, compact.generation()).unwrap();
        let mut delta_path = path.clone();
        delta_path.set_extension("delta");
        let mut delta = DeltaIndex::open(&delta_path, compact.generation()).unwrap();
        delta
            .apply(DeltaChange::Upsert(record(
                "/docs/a.txt",
                30,
                FileKind::File,
            )))
            .unwrap();
        delta
            .apply(DeltaChange::Remove("/docs/sub/b.txt".into()))
            .unwrap();
        delta.sync().unwrap();
        let overlay = DirectorySummaryOverlay::from_delta(summary, &compact, &delta).unwrap();
        let docs = overlay.get(0, "/docs").unwrap().unwrap();
        assert_eq!(docs.logical_bytes, 30);
        let children = overlay.children(0, "/docs").unwrap();
        let file = children
            .iter()
            .find(|child| child.path.as_ref() == "/docs/a.txt")
            .unwrap();
        assert_eq!(file.logical_bytes, 30);
        let sub = children
            .iter()
            .find(|child| child.path.as_ref() == "/docs/sub")
            .unwrap();
        assert_eq!(sub.logical_bytes, 0);
        drop(delta);
        drop(compact);
        cleanup(&path);
        remove_delta(&path);
    }

    #[test]
    fn adjusted_entries_project_base_plus_delta() {
        let path =
            std::env::temp_dir().join(format!("neutra-dir-adjusted-{}.nsx", std::process::id()));
        cleanup(&path);
        let records = vec![
            record("/docs", 0, FileKind::Dir),
            record("/docs/a.txt", 10, FileKind::File),
        ];
        CompactIndex::build_with_summary(&records, &path).unwrap();
        let compact = CompactIndex::open(&path).unwrap();
        let summary = DirectorySummary::open_for_compact(&path, compact.generation()).unwrap();
        let mut delta_path = path.clone();
        delta_path.set_extension("delta");
        let mut delta = DeltaIndex::open(&delta_path, compact.generation()).unwrap();
        delta
            .apply(DeltaChange::Upsert(record(
                "/docs/b.txt",
                5,
                FileKind::File,
            )))
            .unwrap();
        delta.sync().unwrap();
        let overlay = DirectorySummaryOverlay::from_delta(summary, &compact, &delta).unwrap();
        let docs = overlay
            .adjusted_entries()
            .unwrap()
            .into_iter()
            .find(|entry| entry.path.as_ref() == "/docs")
            .unwrap();
        assert_eq!(docs.logical_bytes, 15);
        assert_eq!(docs.file_count, 2);
        drop(delta);
        drop(compact);
        cleanup(&path);
        remove_delta(&path);
    }
}
