//! neutra-core: shared types, in-memory index, query engine, wire protocol.
//!
//! Design rules:
//! - No filesystem walking anywhere in this workspace. Index sources are
//!   filesystem-native metadata structures (NTFS $MFT, ext4 inode/dir blocks
//!   via libext2fs, Btrfs TREE_SEARCH ioctl, ZFS snapshot ZAP enumeration) or
//!   a remote neutrasearch-helper for network mounts.
//! - The index is filename/metadata only (Everything/FSearch scope).

pub mod compact;
pub(crate) mod compact_build;
pub(crate) mod compact_merge;
pub(crate) mod compact_spill;
pub(crate) mod compact_stream;
pub(crate) mod compact_summary;
pub mod delta;
pub mod matcher;
pub mod dir_overlay;
pub mod dir_summary;
pub mod index;
pub mod mounts;
pub mod paths;
pub mod proto;
pub mod query;
pub mod types;

 pub use compact::{join_child_path, CompactIndex, DirFile, DirListing, DirSubdir};
pub use compact_build::BuildStats as CompactBuildStats;
  pub use compact_spill::{SpillAccumulator, SpillRuns};
  pub use delta::{DeltaChange, DeltaIndex, DEFAULT_COMPACT_AT, DELTA_HEADER_BYTES};
 pub use dir_overlay::DirectorySummaryOverlay;
  pub use dir_summary::{
      aggregate_records, DirectoryChild, DirectorySummary, DirectorySummaryEntry,
  };
pub use index::{Index, SearchHit, SearchStats};
pub use mounts::{FsKind, MountInfo, MountSource};
pub use matcher::QueryMatcher;
pub use query::{
    MatchFields, Query, SortKey, ARCHIVE_EXTS, AUDIO_EXTS, DOC_EXTS, EXEC_EXTS, IMAGE_EXTS,
    VIDEO_EXTS,
};
pub use types::{FileKind, FileRecord, ScanStats};
