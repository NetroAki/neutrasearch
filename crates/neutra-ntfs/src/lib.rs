//! NTFS lane: parse `$MFT` directly; never enumerate the mounted namespace.
//! A raw volume that will not open is a hard error: walking millions of
//! files is never an acceptable fallback (slow, hammers the drive), so an
//! unreadable volume means the setup is wrong, never the drive.
//!
//! Supported: boot geometry, MFT data runs (including fragmentation), USA
//! fixups, resident `$STANDARD_INFORMATION` and `$FILE_NAME`. Alternate data
//! streams are intentionally ignored. Records whose only usable name is in an
//! unresolved `$ATTRIBUTE_LIST` are counted as skipped rather than invented.

mod attr_list;
mod geometry;
mod records;

use anyhow::{bail, Context, Result};
use geometry::{apply_fixup, mft_runs, read_exact_at, read_geometry, u16le, u64le, Run};
use neutra_core::{FileKind, FileRecord, FsKind, MountInfo, ScanStats};
use records::{
    append_component, ensure_dir_path, parse_record, Alias, Entry, InvalidFilenameEncoding,
};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::time::Instant;

pub fn scan(mount: &MountInfo, sink: &mut dyn FnMut(FileRecord)) -> Result<ScanStats> {
    let path = volume_path(mount);
    let file = std::fs::File::open(&path).with_context(|| {
        format!(
            "open NTFS volume {path} (run 'neutrasearch index' with root/administrator privileges)"
        )
    })?;
    let volume_size = file.metadata().map(|m| m.len()).unwrap_or(0);
    scan_reader(file, volume_size, &mount.mountpoint.to_string_lossy(), sink)
}
pub(crate) fn volume_path(mount: &MountInfo) -> String {
    #[cfg(target_os = "windows")]
    {
        let d = mount.device.trim_end_matches('\\');
        if d.starts_with(r"\\.\") {
            d.to_string()
        } else {
            format!(r"\\.\{}", d)
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        mount.device.clone()
    }
}
pub fn scan_reader<R: Read + Seek>(
    r: R,
    _volume_size: u64,
    prefix: &str,
    sink: &mut dyn FnMut(FileRecord),
) -> Result<ScanStats> {
    let started = Instant::now();
    let mut r = std::io::BufReader::with_capacity(8 * 1024 * 1024, r);
    let g = read_geometry(&mut r)?;
    let mut rec0 = vec![0u8; g.record as usize];
    read_exact_at(&mut r, g.mft_offset, &mut rec0)?;
    apply_fixup(&mut rec0, g.sector as usize)?;
    let (runs, mft_size) = mft_runs(&rec0, g.cluster)?;
    if runs.is_empty() {
        bail!("$MFT has no non-resident unnamed $DATA runs");
    }
    let runs = attr_list::complete_mft_runs(&mut r, &rec0, g.cluster, g.record, g.sector, runs)?;
    let covered: u64 = runs.iter().map(|run| run.len).sum();
    if covered < mft_size {
        bail!("$MFT runs cover {} of {} declared bytes", covered, mft_size);
    }
    let record_count = mft_size / g.record;
    if record_count > 100_000_000 {
        bail!("implausible MFT record count {record_count}");
    }

    let mut entries = HashMap::<u64, Entry>::with_capacity(record_count.min(4_000_000) as usize);
    let mut aliases = HashMap::<u64, Vec<Alias>>::new();
    let mut skipped_attr_list = 0u64;
    let mut buf = vec![0u8; g.record as usize];
    let mut run_cursor = RunCursor::default();
    for n in 0..record_count {
        run_cursor
            .read(&mut r, &runs, n * g.record, &mut buf)
            .with_context(|| format!("read $MFT record {n}"))?;
        if &buf[..4] != b"FILE" {
            continue;
        }
        let flags = u16le(&buf, 22).unwrap_or(0);
        if flags & 1 == 0 {
            continue;
        }
        if apply_fixup(&mut buf, g.sector as usize).is_err() {
            continue;
        }
        let base_ref = u64le(&buf, 32).unwrap_or(0);
        match parse_record(&buf, flags & 2 != 0) {
            Ok(Some((entry, mut extra_names, has_attr_list))) => {
                if has_attr_list {
                    skipped_attr_list += 1;
                }
                if base_ref != 0 {
                    let base_id = base_ref & 0x0000_ffff_ffff_ffff;
                    let base_sequence = (base_ref >> 48) as u16;
                    if let Some(base) = entries
                        .get_mut(&base_id)
                        .filter(|base| base_sequence == 0 || base.sequence == base_sequence)
                    {
                        if entry.size > base.size {
                            base.size = entry.size;
                        }
                        if entry.alloc > base.alloc {
                            base.alloc = entry.alloc;
                        }
                        extra_names.push(Alias {
                            parent: entry.parent,
                            parent_sequence: entry.parent_sequence,
                            name: entry.name,
                        });
                        let dest = aliases.entry(base_id).or_default();
                        for alias in extra_names {
                            if !dest.contains(&alias)
                                && !(alias.parent == base.parent && alias.name == base.name)
                            {
                                dest.push(alias);
                            }
                        }
                    }
                    continue;
                }
                if !extra_names.is_empty() {
                    aliases.insert(n, extra_names);
                }
                entries.insert(n, entry);
            }
            Ok(None) => {}
            Err(error) if error.downcast_ref::<InvalidFilenameEncoding>().is_some() => {
                return Err(error).with_context(|| format!("parse $MFT record {n}"));
            }
            Err(_) => continue,
        }
    }

    let mut stats = ScanStats::default();
    // Store one portable path spelling in the index. Windows volume roots arrive
    // as `C:\\`; trimming both separators avoids producing `C:\\/name`.
    let prefix = prefix.trim_end_matches(['/', '\\']);
    let mut dir_paths = HashMap::<u64, String>::new();
    dir_paths.insert(5, String::new());
    for (&id, entry) in &entries {
        if id == 5 {
            continue;
        }
        if !ensure_dir_path(
            entry.parent,
            entry.parent_sequence,
            &entries,
            &mut dir_paths,
        ) {
            continue;
        }
        let parent = dir_paths.get(&entry.parent).unwrap();
        let full = if entry.dir {
            let rel = append_component(parent, &entry.name);
            let mut full = String::with_capacity(prefix.len() + rel.len() + 1);
            full.push_str(prefix);
            full.push('/');
            full.push_str(&rel);
            dir_paths.insert(id, rel);
            full
        } else {
            let mut full =
                String::with_capacity(prefix.len() + parent.len() + entry.name.len() + 2);
            full.push_str(prefix);
            full.push('/');
            if !parent.is_empty() {
                full.push_str(parent);
                full.push('/');
            }
            full.push_str(&entry.name);
            full
        };
        let kind = if entry.dir {
            stats.dirs += 1;
            FileKind::Dir
        } else {
            stats.files += 1;
            FileKind::File
        };
        sink(FileRecord {
            path: full.into_boxed_str(),
            size: entry.size,
            disk: FileRecord::allocated_bytes(entry.alloc),
            mtime: entry.mtime,
            mode: 0,
            kind,
            fs: FsKind::Ntfs,
            native_id: id,
            native_parent: entry.parent,
            source: 0,
        });
        stats.records += 1;
    }
    for (id, names) in aliases {
        let Some(entry) = entries.get(&id) else {
            continue;
        };
        if entry.dir {
            continue;
        }
        for alias in names {
            if !ensure_dir_path(
                alias.parent,
                alias.parent_sequence,
                &entries,
                &mut dir_paths,
            ) {
                continue;
            }
            let parent = dir_paths.get(&alias.parent).unwrap();
            let mut full =
                String::with_capacity(prefix.len() + parent.len() + alias.name.len() + 2);
            full.push_str(prefix);
            full.push('/');
            if !parent.is_empty() {
                full.push_str(parent);
                full.push('/');
            }
            full.push_str(&alias.name);
            sink(FileRecord {
                path: full.into_boxed_str(),
                size: entry.size,
                disk: FileRecord::allocated_bytes(entry.alloc),
                mtime: entry.mtime,
                mode: 0,
                kind: FileKind::File,
                fs: FsKind::Ntfs,
                native_id: id,
                native_parent: alias.parent,
                source: 0,
            });
            stats.files += 1;
            stats.records += 1;
        }
    }
    stats.wall_ms = started.elapsed().as_millis() as u64;
    stats.detail = format!(
        "$MFT records={record_count}, runs={}, attr-list records={skipped_attr_list}",
        runs.len()
    );
    Ok(stats)
}

mod cursor;
pub(crate) use cursor::RunCursor;
#[cfg(test)]
mod tests;
