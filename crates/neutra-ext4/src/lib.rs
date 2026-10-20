//! ext2/3/4 lane using libext2fs directly on a block device or image.
//!
//! This performs an inode-table scan and directory-block iteration through
//! libext2fs. It never reads the mounted namespace.

#[cfg(not(target_os = "linux"))]
use anyhow::Result;
#[cfg(not(target_os = "linux"))]
use neutra_core::{FileRecord, MountInfo, ScanStats};
#[cfg(not(target_os = "linux"))]
use std::path::Path;

#[cfg(target_os = "linux")]
#[path = "lib/adapters.rs"]
mod adapters;
#[cfg(target_os = "linux")]
#[path = "lib/domain.rs"]
mod domain;
#[cfg(all(test, target_os = "linux"))]
#[path = "lib/tests.rs"]
mod tests;
#[cfg(target_os = "linux")]
#[path = "lib/view.rs"]
mod view;

#[cfg(target_os = "linux")]
pub use view::{scan, scan_image};
#[cfg(not(target_os = "linux"))]
pub fn scan(_mount: &MountInfo, _sink: &mut dyn FnMut(FileRecord)) -> Result<ScanStats> {
    anyhow::bail!("libext2fs lane is available on Linux builds")
}
#[cfg(not(target_os = "linux"))]
pub fn scan_image(
    _path: &Path,
    _prefix: &str,
    _sink: &mut dyn FnMut(FileRecord),
) -> Result<ScanStats> {
    anyhow::bail!("libext2fs lane is available on Linux builds")
}
