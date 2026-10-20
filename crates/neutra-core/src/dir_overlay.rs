//! The live directory-summary projection: base sidecar + WAL overlay with
//! adjusted totals, direct children, and full adjusted projections. Split
//! from the sidecar format so reading/writing stays separate from deltas.

use crate::dir_summary::{
    ancestor_paths, canonical_path, compare_paths, entry_key, invalid, normalize_path, parent_path,
    ChildKindRank, DirectoryChild, DirectorySummary, DirectorySummaryEntry, Key,
};
use crate::{CompactIndex, DeltaChange, DeltaIndex, FileKind, FileRecord};
use std::collections::{HashMap, HashSet};
mod prefix;
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
    removed_prefixes: Vec<String>,
    live_directories: HashSet<Key>,
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
            removed_prefixes: Vec::new(),
            live_directories: HashSet::new(),
            adjustments: HashMap::new(),
            child_changes: HashMap::new(),
        };
        delta.for_each_removed_prefix(|path| {
            let previous = compact.records_by_path(&path)?;
            overlay.remove_prefix(&path, &previous)
        })?;
        delta.for_each_upsert(|record| {
            let previous = if overlay.base_removed(record.path.as_ref()) {
                Vec::new()
            } else {
                compact.records_by_path(record.path.as_ref())?
            };
            overlay.apply_change(&DeltaChange::Upsert(record.clone()), &previous)?;
            Ok(())
        })?;
        delta.for_each_removed(|path| {
            if overlay.base_removed(&path) {
                return Ok(());
            }
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
            DeltaChange::RemovePrefix(path) => self.remove_prefix(path, previous)?,
            DeltaChange::Upsert(record) => {
                for previous in previous {
                    self.adjust_record(previous, -1)?;
                    self.set_child(previous, None)?;
                }
                self.adjust_record(record, 1)?;
                if record.kind == FileKind::Dir {
                    self.live_directories
                        .insert(entry_key(record.source, &normalize_path(&record.path)?));
                }
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
                    self.live_directories
                        .remove(&entry_key(previous.source, &previous_path));
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
        let base = if self.base_removed(normalized) {
            None
        } else {
            self.base.get(source, normalized).cloned()
        };
        let delta = self
            .adjustments
            .get(&entry_key(source, normalized))
            .copied()
            .unwrap_or_default();
        if base.is_none()
            && !self
                .live_directories
                .contains(&entry_key(source, normalized))
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
mod tests;

mod children;
