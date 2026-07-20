//! NTFS lane: parse `$MFT` directly; never enumerate the mounted namespace.
//!
//! Supported: boot geometry, MFT data runs (including fragmentation), USA
//! fixups, resident `$STANDARD_INFORMATION` and `$FILE_NAME`. Alternate data
//! streams are intentionally ignored. Records whose only usable name is in an
//! unresolved `$ATTRIBUTE_LIST` are counted as skipped rather than invented.

use anyhow::{bail, Context, Result};
use neutra_core::{FileKind, FileRecord, FsKind, MountInfo, ScanStats};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::time::Instant;

#[derive(Debug, Clone, Copy)]
struct Geometry {
    sector: u64,
    cluster: u64,
    record: u64,
    mft_offset: u64,
}
#[derive(Debug, Clone, Copy)]
struct Run {
    logical: u64,
    len: u64,
    physical: u64,
}
#[derive(Debug, Clone)]
struct Entry {
    parent: u64,
    parent_sequence: u16,
    sequence: u16,
    name: String,
    size: u64,
    mtime: i64,
    dir: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct Alias {
    parent: u64,
    parent_sequence: u16,
    name: String,
}

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

fn volume_path(mount: &MountInfo) -> String {
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
    let record_count = mft_size / g.record;
    if record_count > 100_000_000 {
        bail!("implausible MFT record count {record_count}");
    }

    let mut entries = HashMap::<u64, Entry>::with_capacity(record_count.min(4_000_000) as usize);
    let mut aliases = HashMap::<u64, Vec<Alias>>::new();
    let progress = std::env::var_os("NEUTRASEARCH_PROGRESS").is_some()
        || std::env::var_os("NEUTRA_PROGRESS").is_some();
    let mut skipped_attr_list = 0u64;
    let mut buf = vec![0u8; g.record as usize];
    let mut run_cursor = RunCursor::default();
    for n in 0..record_count {
        run_cursor
            .read(&mut r, &runs, n * g.record, &mut buf)
            .with_context(|| format!("read $MFT record {n}"))?;
        if progress && n > 0 && n % 1_000_000 == 0 {
            eprintln!(
                "ntfs records={n}/{record_count} live={} wall_ms={}",
                entries.len(),
                started.elapsed().as_millis()
            );
        }
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
            Err(_) => continue,
        }
    }

    if progress {
        eprintln!(
            "ntfs metadata phase live={} wall_ms={}",
            entries.len(),
            started.elapsed().as_millis()
        );
    }
    let mut stats = ScanStats::default();
    let prefix = prefix.trim_end_matches('/');
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

fn read_geometry<R: Read + Seek>(r: &mut R) -> Result<Geometry> {
    let mut b = [0u8; 512];
    read_exact_at(r, 0, &mut b)?;
    if &b[3..7] != b"NTFS" {
        bail!("not an NTFS boot sector");
    }
    let sector = u16::from_le_bytes([b[11], b[12]]) as u64;
    let spc = b[13] as u64;
    if !sector.is_power_of_two() || spc == 0 {
        bail!("invalid NTFS geometry");
    }
    let cluster = sector.checked_mul(spc).context("cluster overflow")?;
    let mft_lcn = u64::from_le_bytes(b[48..56].try_into().unwrap());
    let encoded = b[64] as i8;
    let record = if encoded < 0 {
        1u64.checked_shl((-encoded) as u32)
            .context("record shift")?
    } else {
        cluster
            .checked_mul(encoded as u64)
            .context("record size overflow")?
    };
    if record < sector || record > 64 * 1024 {
        bail!("invalid MFT record size {record}");
    }
    Ok(Geometry {
        sector,
        cluster,
        record,
        mft_offset: mft_lcn * cluster,
    })
}

fn mft_runs(rec: &[u8], cluster: u64) -> Result<(Vec<Run>, u64)> {
    let mut p = u16le(rec, 20).context("attribute offset")? as usize;
    while p + 16 <= rec.len() {
        let typ = u32le(rec, p).unwrap();
        if typ == 0xffff_ffff {
            break;
        }
        let len = u32le(rec, p + 4).context("attribute length")? as usize;
        if len < 16 || p + len > rec.len() {
            bail!("invalid MFT attribute length");
        }
        let nonresident = rec[p + 8] != 0;
        let name_len = rec[p + 9];
        if typ == 0x80 && nonresident && name_len == 0 {
            if len < 64 {
                bail!("short non-resident $MFT DATA attribute");
            }
            let run_off = u16le(rec, p + 32).unwrap() as usize;
            if run_off < 64 || run_off > len {
                bail!("invalid $MFT runlist offset");
            }
            let real_size = u64le(rec, p + 48).unwrap();
            let runs = parse_runlist(&rec[p + run_off..p + len], cluster)?;
            return Ok((runs, real_size));
        }
        p += len;
    }
    bail!("$MFT unnamed non-resident DATA attribute not found")
}

fn parse_runlist(data: &[u8], cluster: u64) -> Result<Vec<Run>> {
    let mut out = Vec::new();
    let mut p = 0usize;
    let mut lcn = 0i64;
    let mut logical = 0u64;
    while p < data.len() && data[p] != 0 {
        let head = data[p];
        p += 1;
        let ls = (head & 0xf) as usize;
        let os = (head >> 4) as usize;
        if ls == 0 || ls > 8 || os > 8 || p + ls + os > data.len() {
            bail!("invalid NTFS data run");
        }
        let clusters = read_unsigned(&data[p..p + ls]);
        p += ls;
        let delta = read_signed(&data[p..p + os]);
        p += os;
        if os == 0 {
            bail!("sparse $MFT run is invalid");
        }
        lcn = lcn.checked_add(delta).context("MFT LCN overflow")?;
        if lcn < 0 {
            bail!("negative MFT LCN");
        }
        let len = clusters
            .checked_mul(cluster)
            .context("run length overflow")?;
        out.push(Run {
            logical,
            len,
            physical: (lcn as u64) * cluster,
        });
        logical += len;
    }
    Ok(out)
}

fn parse_record(rec: &[u8], header_dir: bool) -> Result<Option<(Entry, Vec<Alias>, bool)>> {
    let mut p = u16le(rec, 20).context("attribute offset")? as usize;
    let sequence = u16le(rec, 16).context("record sequence")?;
    let mut mtime = 0i64;
    let mut best: Option<(u8, Entry)> = None;
    let mut link_names = Vec::<Alias>::new();
    let mut attr_list = false;
    let mut data_size = None::<u64>;
    while p + 16 <= rec.len() {
        let typ = u32le(rec, p).unwrap();
        if typ == 0xffff_ffff {
            break;
        }
        let len = u32le(rec, p + 4).context("attr len")? as usize;
        if len < 16 || p + len > rec.len() {
            break;
        }
        let resident = rec[p + 8] == 0;
        if typ == 0x20 {
            attr_list = true;
        }
        if !resident && typ == 0x80 && rec[p + 9] == 0 && len >= 64 {
            data_size = u64le(rec, p + 48);
        }
        if resident {
            let value_len = u32le(rec, p + 16).unwrap_or(0) as usize;
            let value_off = u16le(rec, p + 20).unwrap_or(0) as usize;
            if value_off + value_len <= len {
                let v = &rec[p + value_off..p + value_off + value_len];
                if typ == 0x80 && rec[p + 9] == 0 {
                    data_size = Some(value_len as u64);
                }
                if typ == 0x10 && v.len() >= 16 {
                    mtime = filetime_to_unix(u64le(v, 8).unwrap());
                }
                if typ == 0x30 && v.len() >= 66 {
                    let parent_ref = u64le(v, 0).unwrap();
                    let parent = parent_ref & 0x0000_ffff_ffff_ffff;
                    let parent_sequence = (parent_ref >> 48) as u16;
                    let size = u64le(v, 48).unwrap_or(0);
                    let flags = u32le(v, 56).unwrap_or(0);
                    let nl = v[64] as usize;
                    let ns = v[65];
                    if 66 + nl * 2 <= v.len() {
                        let words =
                            (0..nl).map(|i| u16::from_le_bytes([v[66 + i * 2], v[67 + i * 2]]));
                        let name = std::char::decode_utf16(words)
                            .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
                            .collect::<String>();
                        if name != "." && name != ".." {
                            let rank = match ns {
                                1 | 3 => 3,
                                0 => 2,
                                2 => 1,
                                _ => 0,
                            };
                            let e = Entry {
                                parent,
                                parent_sequence,
                                sequence,
                                name,
                                size,
                                mtime,
                                dir: header_dir || flags & 0x1000_0000 != 0,
                            };
                            if ns != 2
                                && !link_names
                                    .iter()
                                    .any(|a| a.parent == e.parent && a.name == e.name)
                            {
                                link_names.push(Alias {
                                    parent: e.parent,
                                    parent_sequence: e.parent_sequence,
                                    name: e.name.clone(),
                                });
                            }
                            if best.as_ref().is_none_or(|(r, _)| rank > *r) {
                                best = Some((rank, e));
                            }
                        }
                    }
                }
            }
        }
        p += len;
    }
    if let Some((_, mut e)) = best {
        e.mtime = mtime;
        if let Some(size) = data_size {
            e.size = size;
        }
        link_names.retain(|alias| alias.parent != e.parent || alias.name != e.name);
        Ok(Some((e, link_names, attr_list)))
    } else {
        Ok(None)
    }
}

fn ensure_dir_path(
    id: u64,
    expected_sequence: u16,
    entries: &HashMap<u64, Entry>,
    cache: &mut HashMap<u64, String>,
) -> bool {
    if expected_sequence != 0
        && entries
            .get(&id)
            .is_none_or(|entry| entry.sequence != expected_sequence)
    {
        return false;
    }
    if cache.contains_key(&id) {
        return true;
    }
    let mut current = id;
    let mut chain = Vec::<u64>::new();
    let mut seen = HashSet::new();
    while !cache.contains_key(&current) {
        if !seen.insert(current) {
            return false;
        }
        let Some(e) = entries.get(&current) else {
            return false;
        };
        if !e.dir {
            return false;
        }
        chain.push(current);
        if e.parent_sequence != 0
            && entries
                .get(&e.parent)
                .is_none_or(|parent| parent.sequence != e.parent_sequence)
        {
            return false;
        }
        current = e.parent;
    }
    let mut path = cache.get(&current).unwrap().clone();
    for ino in chain.into_iter().rev() {
        path = append_component(&path, &entries[&ino].name);
        cache.insert(ino, path.clone());
    }
    true
}
fn append_component(parent: &str, name: &str) -> String {
    let mut p = String::with_capacity(parent.len() + name.len() + 1);
    p.push_str(parent);
    if !parent.is_empty() {
        p.push('/');
    }
    p.push_str(name);
    p
}

#[derive(Default)]
struct RunCursor {
    idx: usize,
    logical: u64,
    positioned: bool,
}
impl RunCursor {
    fn read<R: Read + Seek>(
        &mut self,
        r: &mut R,
        runs: &[Run],
        logical: u64,
        out: &mut [u8],
    ) -> Result<()> {
        if !self.positioned || self.logical != logical {
            self.idx = runs
                .iter()
                .position(|x| logical >= x.logical && logical < x.logical + x.len)
                .context("MFT logical hole")?;
            let run = runs[self.idx];
            r.seek(SeekFrom::Start(run.physical + logical - run.logical))?;
            self.logical = logical;
            self.positioned = true;
        }
        let mut done = 0usize;
        while done < out.len() {
            let run = runs.get(self.idx).context("MFT run exhausted")?;
            let within = self.logical - run.logical;
            if within >= run.len {
                self.idx += 1;
                self.positioned = false;
                continue;
            }
            let n = ((run.len - within) as usize).min(out.len() - done);
            r.read_exact(&mut out[done..done + n])?;
            done += n;
            self.logical += n as u64;
            if self.logical == run.logical + run.len {
                self.idx += 1;
                if let Some(next) = runs.get(self.idx) {
                    r.seek(SeekFrom::Start(next.physical))?;
                    self.positioned = true;
                }
            }
        }
        Ok(())
    }
}
fn apply_fixup(rec: &mut [u8], sector: usize) -> Result<()> {
    let usa_off = u16le(rec, 4).context("USA offset")? as usize;
    let count = u16le(rec, 6).context("USA count")? as usize;
    if sector == 0
        || !rec.len().is_multiple_of(sector)
        || count != rec.len() / sector + 1
        || usa_off + count * 2 > rec.len()
    {
        bail!("invalid USA");
    }
    let sig = [rec[usa_off], rec[usa_off + 1]];
    for i in 1..count {
        let end = i * sector;
        if rec[end - 2..end] != sig {
            bail!("USA mismatch");
        }
        let src = usa_off + i * 2;
        rec[end - 2] = rec[src];
        rec[end - 1] = rec[src + 1];
    }
    Ok(())
}
fn read_exact_at<R: Read + Seek>(r: &mut R, off: u64, out: &mut [u8]) -> Result<()> {
    r.seek(SeekFrom::Start(off))?;
    r.read_exact(out)?;
    Ok(())
}
fn filetime_to_unix(v: u64) -> i64 {
    (v / 10_000_000) as i64 - 11_644_473_600
}
fn read_unsigned(b: &[u8]) -> u64 {
    b.iter()
        .enumerate()
        .fold(0, |a, (i, x)| a | ((*x as u64) << (i * 8)))
}
fn read_signed(b: &[u8]) -> i64 {
    if b.is_empty() {
        return 0;
    }
    let u = read_unsigned(b);
    let bits = b.len() * 8;
    if b[b.len() - 1] & 0x80 != 0 {
        (u | (!0u64 << bits)) as i64
    } else {
        u as i64
    }
}
fn u16le(b: &[u8], p: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(p..p + 2)?.try_into().ok()?))
}
fn u32le(b: &[u8], p: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(p..p + 4)?.try_into().ok()?))
}
fn u64le(b: &[u8], p: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(p..p + 8)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn runlist_fragmented() {
        let r = parse_runlist(&[0x11, 3, 5, 0x11, 2, 0xfe, 0], 4096).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].physical, 5 * 4096);
        assert_eq!(r[1].physical, 3 * 4096);
    }
    #[test]
    fn usa_repairs() {
        let mut r = vec![0u8; 1024];
        r[4..6].copy_from_slice(&48u16.to_le_bytes());
        r[6..8].copy_from_slice(&3u16.to_le_bytes());
        r[48..50].copy_from_slice(&[0xaa, 0xbb]);
        r[50..52].copy_from_slice(&[1, 2]);
        r[52..54].copy_from_slice(&[3, 4]);
        r[510..512].copy_from_slice(&[0xaa, 0xbb]);
        r[1022..1024].copy_from_slice(&[0xaa, 0xbb]);
        apply_fixup(&mut r, 512).unwrap();
        assert_eq!(&r[510..512], &[1, 2]);
        assert_eq!(&r[1022..1024], &[3, 4]);
    }
    #[test]
    fn path_cycles_stop() {
        let mut e = HashMap::new();
        e.insert(
            6,
            Entry {
                parent: 5,
                parent_sequence: 0,
                sequence: 1,
                name: "a".into(),
                size: 0,
                mtime: 0,
                dir: true,
            },
        );
        e.insert(
            7,
            Entry {
                parent: 6,
                parent_sequence: 1,
                sequence: 1,
                name: "b".into(),
                size: 0,
                mtime: 0,
                dir: false,
            },
        );
        let mut c = HashMap::from([(5, String::new())]);
        assert!(ensure_dir_path(6, 1, &e, &mut c));
        assert_eq!(append_component(c.get(&6).unwrap(), "b"), "a/b");
        e.get_mut(&6).unwrap().parent = 7;
        let mut c = HashMap::from([(5, String::new())]);
        assert!(!ensure_dir_path(6, 1, &e, &mut c));
    }
}
