//! macOS lane: read the namespace supplied by Spotlight.
//!
//! `mdfind` supplies the namespace from Spotlight, then `symlink_metadata`
//! obtains attributes for each already-known path. This is not directory
//! walking. Missing or failed Spotlight data is an error.

#[cfg(not(target_os = "macos"))]
use anyhow::bail;
use anyhow::Result;
use neutra_core::{FileRecord, MountInfo, ScanStats};

/// Spotlight query matching every indexed filesystem object. `public.item`
/// is the root UTI for files and folders; `-0` preserves embedded newlines.
pub const SPOTLIGHT_QUERY: &str = "kMDItemContentTypeTree == 'public.item'";

pub fn parse_mdfind_nul(bytes: &[u8]) -> Vec<String> {
    bytes
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .filter_map(|s| std::str::from_utf8(s).ok())
        .map(str::to_owned)
        .collect()
}

#[cfg(any(target_os = "macos", all(test, unix)))]
mod spotlight;

#[cfg(target_os = "macos")]
pub fn scan(mount: &MountInfo, sink: &mut dyn FnMut(FileRecord)) -> Result<ScanStats> {
    use neutra_core::FileKind;
    use std::os::unix::fs::MetadataExt;
    use std::process::Command;
    use std::time::Instant;

    let started = Instant::now();
    let output = Command::new("/usr/bin/mdfind")
        .arg("-0")
        .arg("-onlyin")
        .arg(&mount.mountpoint)
        .arg(SPOTLIGHT_QUERY)
        .output();
    let paths = spotlight::paths(output)?;

    let mut stats = ScanStats::default();
    for path in paths {
        let md = match std::fs::symlink_metadata(&path) {
            Ok(md) => md,
            // Spotlight can list a path that was removed after the query ran.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => bail!("cannot read metadata for {path}: {error}"),
        };
        let kind = if md.file_type().is_dir() {
            stats.dirs += 1;
            FileKind::Dir
        } else if md.file_type().is_symlink() {
            FileKind::Symlink
        } else if md.file_type().is_file() {
            stats.files += 1;
            FileKind::File
        } else {
            FileKind::Other
        };
        sink(FileRecord {
            path: path.into_boxed_str(),
            size: md.len(),
            disk: FileRecord::allocated_bytes(md.blocks().saturating_mul(512)),
            mtime: md.mtime(),
            mode: md.mode(),
            kind,
            fs: mount.fs.clone(),
            native_id: 0,
            native_parent: 0,
            source: 0,
        });
        stats.records += 1;
    }
    stats.wall_ms = started.elapsed().as_millis() as u64;
    stats.detail = "Spotlight index (mdfind namespace + one metadata lookup per hit)".into();
    Ok(stats)
}

#[cfg(not(target_os = "macos"))]
pub fn scan(_mount: &MountInfo, _sink: &mut dyn FnMut(FileRecord)) -> Result<ScanStats> {
    bail!("macOS Spotlight lane is only available on macOS")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nul_paths_without_losing_newlines() {
        let p = parse_mdfind_nul(b"/A/a file\0/A/with\nnewline\0");
        assert_eq!(p, vec!["/A/a file", "/A/with\nnewline"]);
    }
}
