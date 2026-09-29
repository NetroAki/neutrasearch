//! $MFT record interpretation: name/parent extraction with Windows name
//! ranking, hardlink aliases, and parent-chain path resolution.

use anyhow::Context as _;
use crate::geometry::{u16le, u32le, u64le};
use anyhow::Result;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub(crate) struct Entry {
    pub parent: u64,
    pub parent_sequence: u16,
    pub sequence: u16,
    pub name: String,
    pub size: u64,
    pub alloc: u64,
    pub mtime: i64,
    pub dir: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Alias {
    pub parent: u64,
    pub parent_sequence: u16,
    pub name: String,
}







pub(crate) fn parse_record(rec: &[u8], header_dir: bool) -> Result<Option<(Entry, Vec<Alias>, bool)>> {
    let mut p = u16le(rec, 20).context("attribute offset")? as usize;
    let sequence = u16le(rec, 16).context("record sequence")?;
    let mut mtime = 0i64;
    let mut best: Option<(u8, Entry)> = None;
    let mut link_names = Vec::<Alias>::new();
    let mut attr_list = false;
    let mut data_size = None::<u64>;
    let mut data_alloc = None::<u64>;
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
            data_alloc = u64le(rec, p + 40);
            data_size = u64le(rec, p + 48);
        }
        if resident {
            let value_len = u32le(rec, p + 16).unwrap_or(0) as usize;
            let value_off = u16le(rec, p + 20).unwrap_or(0) as usize;
            if value_off + value_len <= len {
                let v = &rec[p + value_off..p + value_off + value_len];
                if typ == 0x80 && rec[p + 9] == 0 {
                    data_alloc = Some(value_len as u64);
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
                                alloc: 0,
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
        if let Some(alloc) = data_alloc {
            e.alloc = alloc;
        }
            e.size = size;
        }
        link_names.retain(|alias| alias.parent != e.parent || alias.name != e.name);
        Ok(Some((e, link_names, attr_list)))
    } else {
        Ok(None)
    }
}

pub(crate) fn ensure_dir_path(
    id: u64,
    expected_sequence: u16,
    entries: &HashMap<u64, Entry>,
    cache: &mut HashMap<u64, String>,
) -> bool {
    if id != 5
        && expected_sequence != 0
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
        if e.parent != 5
            && e.parent_sequence != 0
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
pub(crate) fn append_component(parent: &str, name: &str) -> String {
    let mut p = String::with_capacity(parent.len() + name.len() + 1);
    p.push_str(parent);
    if !parent.is_empty() {
        p.push('/');
    }
    p.push_str(name);
    p
}

pub(crate) fn filetime_to_unix(v: u64) -> i64 {
    (v / 10_000_000) as i64 - 11_644_473_600
}
