//! $MFT record interpretation: name/parent extraction with Windows name
//! ranking, hardlink aliases, and parent-chain path resolution.

use crate::geometry::{u16le, u32le, u64le};
use anyhow::Context as _;
use anyhow::Result;

pub(crate) fn prepare_allocated(record: &mut [u8], sector: usize) -> Result<Option<u16>> {
    if record.get(..4) != Some(b"FILE".as_slice()) {
        return Ok(None);
    }
    let flags = u16le(record, 22).context("record allocation flags")?;
    if flags & 1 == 0 {
        return Ok(None);
    }
    crate::geometry::apply_fixup(record, sector)?;
    Ok(Some(flags))
}

#[derive(Debug)]
pub(crate) struct InvalidFilenameEncoding;

impl std::fmt::Display for InvalidFilenameEncoding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NTFS filename contains an unpaired UTF-16 surrogate")
    }
}

impl std::error::Error for InvalidFilenameEncoding {}

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

pub(crate) fn parse_record(
    rec: &[u8],
    header_dir: bool,
) -> Result<Option<(Entry, Vec<Alias>, bool)>> {
    let mut p = u16le(rec, 20).context("attribute offset")? as usize;
    if p > rec.len() {
        anyhow::bail!("attribute offset {p} exceeds record size {}", rec.len());
    }
    let sequence = u16le(rec, 16).context("record sequence")?;
    let mut mtime = 0i64;
    let mut best: Option<(u8, Entry)> = None;
    let mut link_names = Vec::<Alias>::new();
    let mut attr_list = false;
    let mut data_size = None::<u64>;
    let mut data_alloc = None::<u64>;
    loop {
        let Some(header_end) = p.checked_add(4) else {
            anyhow::bail!("attribute header range overflow");
        };
        if header_end > rec.len() {
            anyhow::bail!("truncated attribute header at offset {p}");
        }
        let typ = u32le(rec, p).context("attribute type")?;
        if typ == 0xffff_ffff {
            break;
        }
        if p.checked_add(16).is_none_or(|end| end > rec.len()) {
            anyhow::bail!("truncated attribute header at offset {p}");
        }
        let len = u32le(rec, p + 4).context("attr len")? as usize;
        let end = p.checked_add(len).context("attribute range overflow")?;
        if len < 16 || end > rec.len() {
            anyhow::bail!("invalid attribute length {len} at offset {p}");
        }
        let resident = rec[p + 8] == 0;
        if !resident && len < 64 {
            anyhow::bail!("short non-resident attribute header at offset {p}");
        }
        if typ == 0x20 {
            attr_list = true;
        }
        if !resident && typ == 0x80 && rec[p + 9] == 0 && len >= 64 {
            data_alloc = u64le(rec, p + 40);
            data_size = u64le(rec, p + 48);
        }
        if resident {
            if len < 24 {
                anyhow::bail!("short resident attribute header at offset {p}");
            }
            let value_len = u32le(rec, p + 16).context("resident value length")? as usize;
            let value_off = u16le(rec, p + 20).context("resident value offset")? as usize;
            let value_end = value_off
                .checked_add(value_len)
                .context("resident value range overflow")?;
            if value_off < 24 || value_end > len {
                anyhow::bail!(
                    "resident value range {value_off}..{value_end} exceeds attribute length {len}"
                );
            }
            {
                let v = &rec[p + value_off..p + value_end];
                if (typ == 0x30 && v.len() < 66) || (typ == 0x10 && v.len() < 16) {
                    anyhow::bail!("truncated NTFS attribute value");
                }
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
                    if 66 + nl * 2 > v.len() {
                        anyhow::bail!("truncated NTFS filename");
                    }
                    {
                        let words =
                            (0..nl).map(|i| u16::from_le_bytes([v[66 + i * 2], v[67 + i * 2]]));
                        let mut name = String::new();
                        for c in std::char::decode_utf16(words) {
                            name.push(c.map_err(|_| anyhow::Error::new(InvalidFilenameEncoding))?);
                        }
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
        p = end;
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

#[cfg(test)]
mod tests;
