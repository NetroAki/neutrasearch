//! Compact read-optimized index: compressed path blocks plus trigram postings.
//!
//! The base is immutable and mmapped on Unix. Windows snapshots use owned
//! bytes because Windows forbids atomically replacing a file with live mapped
//! views; this keeps compaction compatible with persistent readers.
use crate::compact_spill::SpillAccumulator;
use crate::matcher::compare_records;
use crate::{DeltaIndex, Query};
use crate::{FileRecord, SearchHit, SearchStats};
use rayon::prelude::*;
use std::cmp::Ordering;
use std::collections::HashSet;
use std::io;
use std::time::Instant;
pub use view::{DirFile, DirListing, DirSubdir};

mod adapters;
mod children;
mod domain;
pub(crate) use domain::{collect_trigrams, compare_index_paths, fold_byte, suffix_path};
use domain::{equivalent_index_path, folded_eq, intersect, retain_best};
pub(crate) use format::{binerr, invalid, put_varint};
use format::{checked, get_varint, u16_at, u32_at, u64_at};
mod directory_paths;
mod fold;
mod state;
#[cfg(test)]
mod tests;
mod view;
pub use directory_paths::join_child_path;
use directory_paths::{dir_prefix, starts_with_folded};
mod listing;
mod search;
use format::TableView;
pub struct CompactIndex {
    map: IndexBytes,
    generation: u64,
    record_count: u64,
    blocks: TableView,
    dict: TableView,
    record_version: u32,
    starts: std::sync::OnceLock<Box<[u32]>>,
}

impl CompactIndex {
    pub fn len(&self) -> u64 {
        self.record_count
    }
    pub fn is_empty(&self) -> bool {
        self.record_count == 0
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

#[cfg(not(unix))]
pub(crate) mod atomic_file;

mod format;
use format::IndexBytes;
pub(crate) use format::{
    BlockDesc, DictEntry, BLOCK_RECORDS, CHECKSUM_BYTES, DESC_SIZE, DICT_SIZE, HEADER, MAGIC,
    MIN_READABLE_VERSION, VERSION,
};

pub(crate) mod checksums;
