//! Generation-bound directory summaries derived from native metadata records.
//!
//! This sidecar stores logical folder totals and direct children. It is built
//! from the already-enumerated `FileRecord` stream; it never touches the
//! filesystem tree. The compact search index remains the source of truth for
//! complete records, while this projection lets file-manager views open a
//! folder hierarchy without decoding every compact record.

use crate::{FileKind, FileRecord};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::hash::{Hash, Hasher};
#[cfg(test)]
use std::io::BufWriter;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

pub(crate) const MAGIC: &[u8; 8] = b"NEUDIR01";
pub(crate) const VERSION: u32 = 3;
/// Oldest readable sidecar layout. Version 1 entries lack on-disk totals;
/// they decode through the frozen layout with physical bytes unset.
pub(crate) const MIN_READABLE_VERSION: u32 = 1;
const PREFIX_BYTES: usize = 20;
const TRAILER_BYTES: usize = 12;
/// Upper bound for the compressed sidecar file. A 100M-record host writes a
/// ~1 GiB sidecar, so the cap must admit real files; corrupt input stays
/// contained by the compressed-bytes checksum plus the stream bound below.
const MAX_SIDECAR_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Decompressed payload bound. A 100M-record host holds ~20 GiB of
/// directory entries. Decoding streams without upfront allocation, so the
/// bound only caps how much a corrupt stream may yield.
const MAX_UNCOMPRESSED_BYTES: u64 = 32 * 1024 * 1024 * 1024;

#[derive(Debug, Clone)]
pub(crate) struct PathKey {
    pub(crate) source: u32,
    /// Precomputed canonical form; hashing/equality run per lookup, and
    /// canonicalizing on the fly allocates per hash.
    pub(crate) canonical: String,
}

pub(crate) type Key = PathKey;

impl PartialEq for PathKey {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source && self.canonical == other.canonical
    }
}

impl Eq for PathKey {}

impl Hash for PathKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.source.hash(state);
        self.canonical.hash(state);
    }
}

/// One direct child shown by a folder view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectoryChild {
    pub path: Box<str>,
    pub kind: FileKind,
    /// For files this is the file's logical size; for directories it is the
    /// aggregate logical size of the directory subtree.
    pub logical_bytes: u64,
    #[serde(default)]
    pub physical_bytes: u64,
    pub file_count: u64,
    pub directory_count: u64,
}

/// Aggregate metadata for one indexed directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectorySummaryEntry {
    pub source: u32,
    pub path: Box<str>,
    pub logical_bytes: u64,
    #[serde(default)]
    pub physical_bytes: u64,
    pub file_count: u64,
    pub directory_count: u64,
    pub children: Vec<DirectoryChild>,
}

/// Read-only directory summary projection for one compact-index generation.
#[derive(Debug, Clone)]
pub struct DirectorySummary {
    generation: u64,
    entries: Vec<DirectorySummaryEntry>,
}

impl DirectorySummary {
    /// Build and atomically publish the sidecar next to `index_path`.
    /// Test-only: production builds stream through the spill.
    #[cfg(test)]
    pub fn build(records: &[FileRecord], index_path: &Path, generation: u64) -> io::Result<Self> {
        let order = summary_order(records)?;
        write_sidecar_from_order(records, &order, index_path, generation)?;
        Self::open_for_compact(index_path, generation)
    }

    #[cfg(test)]
    pub(crate) fn build_sidecar_ordered(
        records: &[FileRecord],
        order: &[u32],
        index_path: &Path,
        generation: u64,
    ) -> io::Result<()> {
        write_sidecar_from_order(records, order, index_path, generation)
    }

    /// Open the sidecar only when it belongs to the expected compact base.
    pub fn open_for_compact(index_path: &Path, generation: u64) -> io::Result<Self> {
        require_current_paths(index_path)?;
        let (stored_generation, entries) = read_sidecar(&directory_summary_path(index_path))?;
        if stored_generation != generation {
            return Err(invalid(format!(
                "directory summary generation {} does not match compact index generation {}",
                stored_generation, generation
            )));
        }
        Ok(Self {
            generation: stored_generation,
            entries,
        })
    }

    /// Return the sidecar path associated with a compact index.
    pub fn path_for(index_path: &Path) -> PathBuf {
        directory_summary_path(index_path)
    }

    /// Copy a staged sidecar alongside a published compact base. A missing
    /// staged sidecar removes any destination sidecar so stale totals cannot be
    /// mistaken for current data.
    pub fn publish(
        staged_index: &Path,
        destination_index: &Path,
        expected_generation: u64,
    ) -> io::Result<()> {
        let staged = directory_summary_path(staged_index);
        let destination = directory_summary_path(destination_index);
        if staged.is_file() {
            let bytes = read_sidecar_bytes(&staged)?;
            // Validate the generation and compressed checksum before
            // publication; full entry decoding remains the reader's job.
            let generation = sidecar_generation(&bytes)?;
            if generation != expected_generation {
                return Err(invalid("staged directory summary generation mismatch"));
            }
            drop(bytes);
            storage::link_or_write_atomic(&staged, &destination)?;
        } else {
            remove_if_present(&destination)?;
        }
        Ok(())
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn entries(&self) -> &[DirectorySummaryEntry] {
        &self.entries
    }

    /// Binary-search the sorted sidecar by source and normalized path.
    pub fn get(&self, source: u32, path: &str) -> Option<&DirectorySummaryEntry> {
        let normalized = normalize_path(path).ok()?;
        self.entries
            .binary_search_by(|entry| compare_key(entry.source, &entry.path, source, &normalized))
            .ok()
            .map(|index| &self.entries[index])
    }

    pub fn children(&self, source: u32, path: &str) -> Option<Vec<DirectoryChild>> {
        let entry = self.get(source, path)?;
        let mut children = entry.children.clone();
        for child in &mut children {
            if child.kind != FileKind::Dir {
                continue;
            }
            if let Some(summary) = self.get(source, &child.path) {
                child.logical_bytes = summary.logical_bytes;
                child.physical_bytes = summary.physical_bytes;
                child.file_count = summary.file_count;
                child.directory_count = summary.directory_count;
            }
        }
        Some(children)
    }

    #[cfg(test)]
    fn from_records(records: &[FileRecord], generation: u64) -> io::Result<Self> {
        let mut entries = aggregate_records(records)?;
        entries.sort_unstable_by(|left, right| {
            compare_key(left.source, &left.path, right.source, &right.path)
        });
        Ok(Self {
            generation,
            entries,
        })
    }
}

pub(crate) trait ChildKindRank {
    fn kind_rank(&self) -> u8;
}

impl ChildKindRank for DirectoryChild {
    fn kind_rank(&self) -> u8 {
        u8::from(self.kind != FileKind::Dir)
    }
}

mod aggregate;
mod decode;
mod paths;
mod storage;
#[cfg(test)]
mod test_writer;
#[cfg(test)]
mod tests;

pub use aggregate::aggregate_records;
#[cfg(test)]
use aggregate::emit_records;
pub(crate) use aggregate::{close_stack, open_entry, OpenEntry};
pub(crate) use decode::walk_file_frames;
#[cfg(test)]
use decode::{decode_entry, OldSummaryEntry};
use decode::{read_sidecar, read_sidecar_bytes, sidecar_generation};
use paths::compare_key;
pub(crate) use paths::{
    ancestor_paths, canonical_path, compare_paths, directory_summary_path, entry_key,
    normalize_path, parent_path,
};
use storage::remove_if_present;
pub(crate) use storage::{codec, invalid, open_private_file, temporary_path, HashingWriter};
#[cfg(test)]
use storage::{replace_file, sync_parent};
#[cfg(test)]
use test_writer::write_sidecar_from_order;

#[cfg(test)]
pub(crate) use aggregate::summary_order;

pub(crate) fn require_current_paths(index_path: &Path) -> io::Result<()> {
    let mut file = File::open(directory_summary_path(index_path))?;
    let mut header = [0; 12];
    file.read_exact(&mut header)?;
    if u32::from_le_bytes(header[8..12].try_into().unwrap()) < 3 && !cfg!(windows) {
        return Err(invalid(
            "directory summary uses older path semantics; rebuild the native metadata index",
        ));
    }
    Ok(())
}
