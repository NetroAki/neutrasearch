//! macOS lane: Spotlight is the indexed fast path.
//!
//! `mdfind` supplies the namespace from Spotlight, then `symlink_metadata`
//! obtains attributes for each already-known path. This is not directory
//! walking. If Spotlight is disabled, the future native fallback is
//! `getattrlistbulk(2)`; readdir recursion is never used.

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

#[cfg(target_os = "macos")]
mod bulk;
#[cfg(target_os = "macos")]
use bulk::bulk_fallback;

#[cfg(target_os = "macos")]
pub fn scan(mount: &MountInfo, sink: &mut dyn FnMut(FileRecord)) -> Result<ScanStats> {
    use anyhow::Context as _;
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
    let paths = match output {
        Ok(output) if output.status.success() => parse_mdfind_nul(&output.stdout),
        Ok(output) => {
            let reason = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            return bulk_fallback(mount, sink).with_context(|| {
                format!("Spotlight query failed ({reason}); native bulk fallback also failed")
            });
        }
        Err(error) => {
            return bulk_fallback(mount, sink).with_context(|| {
                format!("cannot launch Spotlight query ({error}); native bulk fallback also failed")
            });
        }
    };
    if paths.is_empty() {
        // Do not parse localized mdutil prose. An empty all-items query is
        // sufficient evidence that Spotlight cannot supply this volume.
        return bulk_fallback(mount, sink)
            .context("Spotlight returned no indexed objects and native bulk fallback failed");
    }

    let mut stats = ScanStats::default();
    for path in paths {
        let Ok(md) = std::fs::symlink_metadata(&path) else {
            continue;
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
