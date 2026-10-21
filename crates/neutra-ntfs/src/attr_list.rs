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
fn attribute_list_extent(rec0: &[u8], cluster_size: u64) -> Result<Option<(Vec<Run>, u64)>> {
    let mut p = u16le(rec0, 20).context("attribute-list offset")? as usize;
    loop {
        let kind = u32le(rec0, p).context("truncated attribute-list header")?;
        if kind == 0xFFFF_FFFF {
            return Ok(None);
        }
        let length = u32le(rec0, p + 4).context("attribute-list length")? as usize;
        let end = p
            .checked_add(length)
            .context("attribute-list range overflow")?;
        if length < 16 || end > rec0.len() {
            bail!("invalid attribute-list attribute length");
        }
        if kind == 0x20 && rec0[p + 8] != 0 {
            if length < 64 {
                bail!("short non-resident attribute-list header");
            }
            let run_offset = u16le(rec0, p + 32).context("attribute-list run offset")? as usize;
            if run_offset < 64 || run_offset > length {
                bail!("invalid attribute-list run offset");
            }
            let real = u64le(rec0, p + 48).context("attribute-list size")?;
            let runs = parse_runlist(&rec0[p + run_offset..end], cluster_size)?;
            return Ok(Some((runs, real)));
        }
        p = end;
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
    if p > record.len() {
        bail!(
            "extension record {} attribute offset exceeds record size",
            entry.record
        );
    }
    loop {
        let header_end = p
            .checked_add(4)
            .context("extension attribute header range overflow")?;
        if header_end > record.len() {
            bail!(
                "extension record {} has a truncated attribute header at {p}",
                entry.record
            );
        }
        let kind = u32le(&record, p).ok_or_else(|| anyhow::anyhow!("attribute type overrun"))?;
        if kind == 0xFFFF_FFFF {
            break;
        }
        if p.checked_add(16).is_none_or(|end| end > record.len()) {
            bail!("truncated extension attribute header");
        }
        let length = u32le(&record, p + 4).context("extension attribute length")? as usize;
        let end = p
            .checked_add(length)
            .context("extension attribute range overflow")?;
        if length < 16 || end > record.len() {
            bail!(
                "extension record {} has invalid attribute length {length} at {p}",
                entry.record
            );
        }
        let non_resident = record[p + 8] != 0;
        let attribute_id = u16le(&record, p + 14).unwrap_or(0);
        let lowest_vcn = u64le(&record, p + 16).unwrap_or(0);
        if kind == 0x80 && non_resident && record[p + 9] == 0 && attribute_id == entry.attribute_id
        {
            if length < 64 {
                bail!(
                    "extension record {} has a short non-resident $DATA header",
                    entry.record
                );
            }
            if lowest_vcn != entry.lowest_vcn {
                bail!(
                    "extension record {} covers vcn {} but the list says {}",
                    entry.record,
                    lowest_vcn,
                    entry.lowest_vcn
                );
            }
            let run_offset = u16le(&record, p + 32).context("extension runlist offset")? as usize;
            if run_offset < 64 || run_offset > length {
                bail!(
                    "extension record {} has invalid runlist offset {run_offset}",
                    entry.record
                );
            }
            let start = p
                .checked_add(run_offset)
                .context("extension runlist start overflow")?;
            return parse_runlist(
                record.get(start..end).context("extension runlist range")?,
                cluster_size,
            );
        }
        p = end;
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
    let Some((list_runs, list_size)) = attribute_list_extent(rec0, cluster_size)? else {
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
mod tests;
