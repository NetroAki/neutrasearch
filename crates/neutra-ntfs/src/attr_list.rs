//! $MFT runlist overflow: on heavily fragmented volumes the $DATA runlist
//! outgrows the base MFT record, and NTFS moves the continuation runs into
//! extension records referenced through a non-resident `$ATTRIBUTE_LIST`.
//! Split from the base parse so geometry and record interpretation stay
//! focused; this module only knows how to follow that list.

use anyhow::{bail, Context, Result};
use std::io::{Read, Seek};

use crate::geometry::{apply_fixup, parse_runlist, u16le, u32le, u64le, Run};
use crate::RunCursor;

/// Locate the non-resident `$ATTRIBUTE_LIST` in record 0 and return
/// `(runs covering its value, value size in bytes)`.
fn attribute_list_extent(rec0: &[u8], cluster_size: u64) -> Option<(Vec<Run>, u64)> {
    let mut p = u16le(rec0, 20)? as usize;
    loop {
        if p + 16 > rec0.len() {
            return None;
        }
        let kind = u32le(rec0, p)?;
        if kind == 0xFFFF_FFFF {
            return None;
        }
        let length = u32le(rec0, p + 4)? as usize;
        if length == 0 || p + length > rec0.len() {
            return None;
        }
        let non_resident = rec0[p + 8] != 0;
        if kind == 0x20 && non_resident {
            let run_offset = u16le(rec0, p + 32)? as usize;
            let real = u64le(rec0, p + 48)?;
            let data = &rec0[p + run_offset..p + length];
            let runs = parse_runlist(data, cluster_size).ok()?;
            return Some((runs, real));
        }
        p += length;
    }
}

/// Declared `$DATA` size of `$MFT` from the base record.
fn mft_data_size(rec0: &[u8]) -> Option<u64> {
    let mut p = u16le(rec0, 20)? as usize;
    loop {
        if p + 16 > rec0.len() {
            return None;
        }
        let kind = u32le(rec0, p)?;
        if kind == 0xFFFF_FFFF {
            return None;
        }
        let length = u32le(rec0, p + 4)? as usize;
        if length == 0 || p + length > rec0.len() {
            return None;
        }
        if kind == 0x80 && rec0[p + 8] != 0 {
            return u64le(rec0, p + 48);
        }
        p += length;
    }
}

struct AttributeListEntry {
    attribute_kind: u32,
    lowest_vcn: u64,
    record: u64,
    sequence: u16,
    attribute_id: u16,
}

/// Decode the attribute-list value using each entry own record length.
/// Nameless entries are 32 bytes: kind@0, length@4, name len@6, name off@7,
/// lowest VCN@8, file reference@16 (low 48 bits segment, high 16 sequence),
/// attribute id@24. There is no instance field.
fn parse_attribute_list_entries(value: &[u8]) -> Vec<AttributeListEntry> {
    let mut entries = Vec::new();
    let mut p = 0usize;
    while p + 26 <= value.len() {
        let length = u16le(value, p + 4).unwrap_or(0) as usize;
        if length < 26 || p + length > value.len() {
            break;
        }
        let attribute_kind = u32le(value, p).unwrap_or(0);
        let lowest_vcn = u64le(value, p + 8).unwrap_or(0);
        let reference = u64le(value, p + 16).unwrap_or(0);
        entries.push(AttributeListEntry {
            attribute_kind,
            lowest_vcn,
            record: reference & 0x0000_FFFF_FFFF_FFFF,
            sequence: (reference >> 48) as u16,
            attribute_id: u16le(value, p + 24).unwrap_or(0),
        });
        p += length;
    }
    entries
}

/// Materialize a non-resident attribute value whose runs are byte-granular
/// (cluster size 1).
fn read_value_bytes<R: Read + Seek>(r: &mut R, runs: &[Run], size: u64) -> Result<Vec<u8>> {
    let mut value = vec![0u8; size as usize];
    let mut done = 0u64;
    for run in runs {
        let take = run.len.min(size - done);
        r.seek(std::io::SeekFrom::Start(run.physical + done - run.logical))?;
        read_exact_at_value(r, &mut value[done as usize..(done + take) as usize])?;
        done += take;
        if done >= size {
            break;
        }
    }
    if done < size {
        bail!("attribute list runs cover {} of {} bytes", done, size);
    }
    Ok(value)
}

fn read_exact_at_value<R: Read>(r: &mut R, out: &mut [u8]) -> Result<()> {
    r.read_exact(out)?;
    Ok(())
}

/// Read one MFT record from the covered prefix of `$MFT` and return the
/// runlist of the `$DATA` extension whose instance matches.
fn extension_record_runs<R: Read + Seek>(
    r: &mut R,
    base_runs: &[Run],
    record_size: u64,
    sector_size: u64,
    cluster_size: u64,
    entry: &AttributeListEntry,
) -> Result<Vec<Run>> {
    let mut cursor = RunCursor::default();
    let mut record = vec![0u8; record_size as usize];
    cursor
        .read(r, base_runs, entry.record * record_size, &mut record)
        .with_context(|| format!("read $MFT extension record {}", entry.record))?;
    apply_fixup(&mut record, sector_size as usize)
        .with_context(|| format!("fix up $MFT extension record {}", entry.record))?;
    let sequence = u16le(&record, 16)
        .with_context(|| format!("extension record {} sequence", entry.record))?;
    if entry.sequence != 0 && sequence != entry.sequence {
        bail!(
            "extension record {} sequence {} does not match the list ({})",
            entry.record,
            sequence,
            entry.sequence
        );
    }
    let mut p = u16le(&record, 20).context("extension record attribute offset")? as usize;
    while p + 16 <= record.len() {
        let kind = u32le(&record, p).ok_or_else(|| anyhow::anyhow!("attribute type overrun"))?;
        if kind == 0xFFFF_FFFF {
            break;
        }
        let length = u32le(&record, p + 4).unwrap_or(0) as usize;
        if length == 0 || p + length > record.len() {
            break;
        }
        let non_resident = record[p + 8] != 0;
        let attribute_id = u16le(&record, p + 14).unwrap_or(0);
        let lowest_vcn = u64le(&record, p + 16).unwrap_or(0);
        if kind == 0x80 && non_resident && record[p + 9] == 0 && attribute_id == entry.attribute_id
        {
            if lowest_vcn != entry.lowest_vcn {
                bail!(
                    "extension record {} covers vcn {} but the list says {}",
                    entry.record,
                    lowest_vcn,
                    entry.lowest_vcn
                );
            }
            let run_offset = u16le(&record, p + 32).context("extension runlist offset")? as usize;
            return parse_runlist(&record[p + run_offset..p + length], cluster_size);
        }
        p += length;
    }
    bail!(
        "extension record {} has no $DATA extension (attribute id {})",
        entry.record,
        entry.attribute_id
    )
}

/// Complete the `$MFT` runlist by following the `$ATTRIBUTE_LIST`, when the
/// base record's runlist covers less than the declared `$DATA` size. Returns
/// the original runs unchanged for well-formed small volumes.
pub(crate) fn complete_mft_runs<R: Read + Seek>(
    r: &mut R,
    rec0: &[u8],
    cluster_size: u64,
    record_size: u64,
    sector_size: u64,
    mut runs: Vec<Run>,
) -> Result<Vec<Run>> {
    let Some((list_runs, list_size)) = attribute_list_extent(rec0, cluster_size) else {
        return Ok(runs);
    };
    let declared_size = mft_data_size(rec0).unwrap_or(0);
    let covered: u64 = runs.iter().map(|run| run.len).sum();
    if covered >= declared_size {
        return Ok(runs);
    }
    let value = read_value_bytes(r, &list_runs, list_size).context("read $ATTRIBUTE_LIST value")?;
    let entries = parse_attribute_list_entries(&value);
    let mut extensions: Vec<_> = entries
        .into_iter()
        .filter(|entry| entry.attribute_kind == 0x80 && entry.lowest_vcn > 0)
        .collect();
    extensions.sort_by_key(|entry| entry.lowest_vcn);
    let mut covered: u64 = runs.iter().map(|run| run.len).sum();
    for entry in &extensions {
        let want = entry
            .lowest_vcn
            .checked_mul(cluster_size)
            .context("extension VCN overflow")?;
        if want != covered {
            bail!(
                "extension record {} starts at byte {} but the runs cover {} bytes",
                entry.record,
                want,
                covered
            );
        }
        let mut extension =
            extension_record_runs(r, &runs, record_size, sector_size, cluster_size, entry)?;
        for run in &mut extension {
            run.logical += want;
        }
        covered += extension.iter().map(|run| run.len).sum::<u64>();
        runs.extend(extension);
    }
    if covered < declared_size {
        bail!(
            "$MFT runs cover {} of {} declared bytes even after the attribute list",
            covered,
            declared_size
        );
    }
    Ok(runs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn test_entry(
        kind: u32,
        lowest_vcn: u64,
        segment: u64,
        sequence: u16,
        attribute_id: u16,
    ) -> [u8; 32] {
        let mut entry = [0u8; 32];
        entry[0..4].copy_from_slice(&kind.to_le_bytes());
        entry[4..6].copy_from_slice(&32u16.to_le_bytes());
        entry[7] = 26;
        entry[8..16].copy_from_slice(&lowest_vcn.to_le_bytes());
        entry[16..24].copy_from_slice(&(segment | ((sequence as u64) << 48)).to_le_bytes());
        entry[24..26].copy_from_slice(&attribute_id.to_le_bytes());
        entry
    }

    #[test]
    fn parses_32byte_nameless_entries() {
        // Ground truth: first continuation entry of the live attribute-list
        // value on the 14.6 TB NTFS volume (LCN 255822480). Stride is 32;
        // the old decoder required 34 and returned nothing.
        let raw = [
            0x80, 0x00, 0x00, 0x00, 0x20, 0x00, 0x00, 0x1a, 0x52, 0x1e, 0x34, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x08, 0x01, 0x00, 0x00, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
        ];
        let mut value = Vec::new();
        value.extend_from_slice(&test_entry(0x10, 0, 0, 1, 0));
        value.extend_from_slice(&test_entry(0x30, 0, 0, 1, 3));
        value.extend_from_slice(&test_entry(0x80, 0, 0, 1, 6));
        value.extend_from_slice(&raw);
        let entries = parse_attribute_list_entries(&value);
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[2].attribute_kind, 0x80);
        assert_eq!(entries[2].lowest_vcn, 0);
        assert_eq!(entries[2].attribute_id, 6);
        let cont = &entries[3];
        assert_eq!(cont.attribute_kind, 0x80);
        assert_eq!(cont.lowest_vcn, 3415634);
        assert_eq!(cont.record, 264);
        assert_eq!(cont.sequence, 9);
        assert_eq!(cont.attribute_id, 0);
    }

    fn nonresident(
        kind: u32,
        attribute_id: u16,
        low_vcn: u64,
        high_vcn: u64,
        real: u64,
        runs: &[u8],
    ) -> Vec<u8> {
        let mut attr = vec![0u8; 64 + runs.len()];
        let total = attr.len() as u32;
        attr[0..4].copy_from_slice(&kind.to_le_bytes());
        attr[4..8].copy_from_slice(&total.to_le_bytes());
        attr[8] = 1;
        attr[14..16].copy_from_slice(&attribute_id.to_le_bytes());
        attr[16..24].copy_from_slice(&low_vcn.to_le_bytes());
        attr[24..32].copy_from_slice(&high_vcn.to_le_bytes());
        attr[32..34].copy_from_slice(&64u16.to_le_bytes());
        attr[40..48].copy_from_slice(&((high_vcn - low_vcn + 1) * 4096).to_le_bytes());
        attr[48..56].copy_from_slice(&real.to_le_bytes());
        attr[56..64].copy_from_slice(&real.to_le_bytes());
        attr[64..].copy_from_slice(runs);
        attr
    }

    fn stamp_usa(rec: &mut [u8; 1024]) {
        rec[0..4].copy_from_slice(b"FILE");
        rec[4..6].copy_from_slice(&48u16.to_le_bytes());
        rec[6..8].copy_from_slice(&3u16.to_le_bytes());
        rec[48..50].copy_from_slice(&[0xaa, 0xbb]);
        rec[50..52].copy_from_slice(&[1, 2]);
        rec[52..54].copy_from_slice(&[3, 4]);
        rec[510..512].copy_from_slice(&[0xaa, 0xbb]);
        rec[1022..1024].copy_from_slice(&[0xaa, 0xbb]);
    }

    /// Fake disk: base data at LCN 10 (2 clusters), attribute-list value at
    /// LCN 20, extension data at LCN 30; extension record is MFT record 5.
    fn fixture(continuation_vcn: u64) -> (Cursor<Vec<u8>>, Vec<u8>, Vec<Run>) {
        let mut disk = vec![0u8; 64 * 4096];
        let mut value = Vec::new();
        value.extend_from_slice(&test_entry(0x80, 0, 0, 1, 3));
        value.extend_from_slice(&test_entry(0x80, continuation_vcn, 5, 7, 5));
        disk[20 * 4096..20 * 4096 + value.len()].copy_from_slice(&value);
        let mut ext = [0u8; 1024];
        stamp_usa(&mut ext);
        ext[16..18].copy_from_slice(&7u16.to_le_bytes());
        ext[20..22].copy_from_slice(&56u16.to_le_bytes());
        let data = nonresident(0x80, 5, 2, 2, 0, &[0x11, 0x01, 0x1e]);
        ext[56..56 + data.len()].copy_from_slice(&data);
        ext[56 + data.len()..56 + data.len() + 4].copy_from_slice(&0xffff_ffffu32.to_le_bytes());
        disk[10 * 4096 + 5 * 1024..10 * 4096 + 6 * 1024].copy_from_slice(&ext);
        let mut rec0 = vec![0u8; 1024];
        rec0[20..22].copy_from_slice(&56u16.to_le_bytes());
        let list = nonresident(0x20, 7, 0, 0, value.len() as u64, &[0x11, 0x01, 0x14]);
        let mft = nonresident(0x80, 3, 0, 1, 12288, &[0x11, 0x02, 0x0a]);
        rec0[56..56 + list.len()].copy_from_slice(&list);
        rec0[56 + list.len()..56 + list.len() + mft.len()].copy_from_slice(&mft);
        let end = 56 + list.len() + mft.len();
        rec0[end..end + 4].copy_from_slice(&0xffff_ffffu32.to_le_bytes());
        let base = parse_runlist(&[0x11, 0x02, 0x0a], 4096).unwrap();
        (Cursor::new(disk), rec0, base)
    }

    #[test]
    fn completes_runs_through_extension_record() {
        let (mut disk, rec0, base) = fixture(2);
        let runs = complete_mft_runs(&mut disk, &rec0, 4096, 1024, 512, base).unwrap();
        assert_eq!(runs.len(), 2);
        assert_eq!(
            (runs[1].logical, runs[1].len, runs[1].physical),
            (8192, 4096, 30 * 4096)
        );
        let covered: u64 = runs.iter().map(|run| run.len).sum();
        assert_eq!(covered, 12288);
    }

    #[test]
    fn gapped_continuation_fails_closed() {
        let (mut disk, rec0, base) = fixture(3);
        assert!(complete_mft_runs(&mut disk, &rec0, 4096, 1024, 512, base).is_err());
    }
}
