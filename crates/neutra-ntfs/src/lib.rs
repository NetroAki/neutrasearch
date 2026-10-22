//! NTFS lane: parse `$MFT` directly; never enumerate the mounted namespace.
//! A raw volume that will not open is a hard error: walking millions of
//! files is never an acceptable fallback (slow, hammers the drive), so an
//! unreadable volume means the setup is wrong, never the drive.
//!
//! Supported: boot geometry, MFT data runs (including fragmentation), USA
//! fixups, resident `$STANDARD_INFORMATION` and `$FILE_NAME`. Alternate data
//! streams are intentionally ignored. Records whose only usable name is in an
//! unresolved `$ATTRIBUTE_LIST` are counted as skipped rather than invented.
//! Named records whose parent chain cannot reach the volume root fail the
//! scan before publication; the staged caller then keeps the old index
//! instead of publishing a silently gapped one.

mod attr_list;
mod extensions;
mod geometry;
mod namespace;
mod records;

use anyhow::{bail, Context, Result};
use geometry::{
    apply_fixup, check_runs_within, device_extent, mft_runs, read_exact_at, read_geometry, u16le,
    u64le, Run,
};
use namespace::{ensure_dir_path, validate};
use neutra_core::{FileKind, FileRecord, FsKind, MountInfo, ScanStats};
use records::{append_component, parse_record, Alias, Entry};
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
    volume_size: u64,
    prefix: &str,
    sink: &mut dyn FnMut(FileRecord),
) -> Result<ScanStats> {
    let started = Instant::now();
    let mut r = std::io::BufReader::with_capacity(8 * 1024 * 1024, r);
    let g = read_geometry(&mut r)?;
    let device = device_extent(&mut r, volume_size, g.declared_bytes)?;
    let mut rec0 = vec![0u8; g.record as usize];
    read_exact_at(&mut r, g.mft_offset, &mut rec0)?;
    apply_fixup(&mut rec0, g.sector as usize)?;
    let (runs, mft_size) = mft_runs(&rec0, g.cluster)?;
    if runs.is_empty() {
        bail!("$MFT has no non-resident unnamed $DATA runs");
    }
    let runs = attr_list::complete_mft_runs(&mut r, &rec0, g.cluster, g.record, g.sector, runs)?;
    check_runs_within(&runs, device)?;
    let covered: u64 = runs.iter().map(|run| run.len).sum();
    if covered < mft_size {
        bail!("$MFT runs cover {} of {} declared bytes", covered, mft_size);
    }
    let record_count = mft_size / g.record;
    if record_count > 100_000_000 || mft_size > device {
        bail!("implausible MFT record count {record_count} on a {device}-byte device");
    }

    let reserve = record_count.min(device / g.record).min(4_000_000);
    let mut entries = HashMap::<u64, Entry>::with_capacity(reserve as usize);
    let mut aliases = HashMap::<u64, Vec<Alias>>::new();
    let mut skipped_attr_list = 0u64;
    let mut root_allocated = false;
    let mut root_dir = false;
    let mut root_sequence = 0u16;
    let mut extensions = Vec::new();
    let mut buf = vec![0u8; g.record as usize];
    let mut run_cursor = RunCursor::default();
    for n in 0..record_count {
        run_cursor
            .read(&mut r, &runs, n * g.record, &mut buf)
            .with_context(|| format!("read $MFT record {n}"))?;
        let Some(flags) = records::prepare_allocated(&mut buf, g.sector as usize)
            .with_context(|| format!("validate allocated $MFT record {n}"))?
        else {
            continue;
        };
        if n == 5 {
            root_allocated = true;
            root_dir = flags & 2 != 0;
            root_sequence = u16le(&buf, 16).unwrap_or(0);
        }
        let base_ref = u64le(&buf, 32).unwrap_or(0);
        match parse_record(&buf, flags & 2 != 0) {
            Ok(Some((entry, extra_names, has_attr_list))) => {
                if has_attr_list {
                    skipped_attr_list += 1;
                }
                if base_ref != 0 {
                    extensions.push((base_ref, entry, extra_names));
                    continue;
                }
                if !extra_names.is_empty() {
                    aliases.insert(n, extra_names);
                }
                entries.insert(n, entry);
            }
            Ok(None) => {}
            Err(error) => return Err(error).with_context(|| format!("parse $MFT record {n}")),
        }
    }

    extensions::merge(extensions, &mut entries, &mut aliases);
    // Fail torn branches before publication so the staged caller keeps the
    // old index instead of a silently gapped one. Intentional omissions
    // are narrow: "."/"..", alternate data streams, DOS-namespace
    // aliases, and names visible only through an unresolved
    // `$ATTRIBUTE_LIST` (counted in `skipped_attr_list`). Everything else
    // named must resolve to the volume root.
    let mut dir_paths = HashMap::<u64, String>::new();
    validate(
        &entries,
        &aliases,
        root_allocated,
        root_dir,
        root_sequence,
        &mut dir_paths,
    )
    .context("unresolved named NTFS branches")?;
    let mut validation_seen = std::collections::HashSet::new();
    let mut stats = ScanStats::default();
    // Store one portable path spelling in the index. Windows volume roots arrive
    // as `C:\\`; trimming both separators avoids producing `C:\\/name`.
    let prefix = prefix.trim_end_matches(['/', '\\']);
    for (&id, entry) in &entries {
        if id == 5 {
            continue;
        }
        ensure_dir_path(
            entry.parent,
            entry.parent_sequence,
            root_sequence,
            &entries,
            &mut dir_paths,
            &mut validation_seen,
        )
        .map_err(|error| {
            anyhow::anyhow!(
                "MFT record {id} (\"{}\") parent path failed after validation: {error}",
                entry.name
            )
        })?;
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
            ensure_dir_path(
                alias.parent,
                alias.parent_sequence,
                root_sequence,
                &entries,
                &mut dir_paths,
                &mut validation_seen,
            )
            .map_err(|error| {
                anyhow::anyhow!(
                    "MFT record {id} alias (\"{}\") parent path failed after validation: {error}",
                    alias.name
                )
            })?;
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
mod test_images;
#[cfg(test)]
mod tests;
