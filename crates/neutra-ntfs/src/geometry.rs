//! NTFS boot-geometry and $MFT runlist parsing: boot sector validation,
//! USA fixup application, and runlist decoding. Split from `records` so
//! byte-level layout logic stays separate from record interpretation.

use anyhow::{bail, Context, Result};
use std::io::{Read, Seek, SeekFrom};

#[derive(Debug, Clone, Copy)]
pub(crate) struct Geometry {
    pub sector: u64,
    pub cluster: u64,
    pub record: u64,
    pub mft_offset: u64,
}
#[derive(Debug, Clone, Copy)]
pub(crate) struct Run {
    pub logical: u64,
    pub len: u64,
    pub physical: u64,
}
pub(crate) fn read_unsigned(b: &[u8]) -> u64 {
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
pub(crate) fn u16le(b: &[u8], p: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(p..p + 2)?.try_into().ok()?))
}
pub(crate) fn u32le(b: &[u8], p: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(p..p + 4)?.try_into().ok()?))
}
pub(crate) fn u64le(b: &[u8], p: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(p..p + 8)?.try_into().ok()?))
}

pub(crate) fn read_geometry<R: Read + Seek>(r: &mut R) -> Result<Geometry> {
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
pub(crate) fn mft_runs(rec: &[u8], cluster: u64) -> Result<(Vec<Run>, u64)> {
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
pub(crate) fn parse_runlist(data: &[u8], cluster: u64) -> Result<Vec<Run>> {
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
pub(crate) fn apply_fixup(rec: &mut [u8], sector: usize) -> Result<()> {
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
pub(crate) fn read_exact_at<R: Read + Seek>(r: &mut R, off: u64, out: &mut [u8]) -> Result<()> {
    r.seek(SeekFrom::Start(off))?;
    r.read_exact(out)?;
    Ok(())
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
}
