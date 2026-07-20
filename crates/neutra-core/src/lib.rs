//! neutra-core: shared types, in-memory index, query engine, wire protocol.
//!
//! Design rules:
//! - No filesystem walking anywhere in this workspace. Index sources are
//!   filesystem-native metadata structures (NTFS $MFT, ext4 inode/dir blocks
//!   via libext2fs, Btrfs TREE_SEARCH ioctl, ZFS snapshot ZAP enumeration) or
//!   a remote neutrasearch-helper for network mounts.
//! - The index is filename/metadata only (Everything/FSearch scope).

pub mod compact;
pub mod delta;
pub mod index;
pub mod mounts;
pub mod proto;
pub mod query;
pub mod types;

pub use compact::{BuildStats as CompactBuildStats, CompactIndex};
pub use delta::{DeltaChange, DeltaIndex, DEFAULT_COMPACT_AT};
pub use index::{Index, SearchHit, SearchStats};
pub use mounts::{FsKind, MountInfo, MountSource};
pub use query::{Query, SortKey};
pub use types::{FileKind, FileRecord, ScanStats};
