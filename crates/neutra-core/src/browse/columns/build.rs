//! One streaming pass over the base builds every column.
use super::write::{write_all_at, Section};
use super::{
    clamp_mtime, pack_attrs, HEADER, INITIAL_ENTRY, INITIAL_KEYS, MAGIC, SIZE_BYTES, SIZE_MAX,
};
use crate::browse::short_counts::Counts;
use crate::CompactIndex;
use std::{fs::File, io, path::Path};

const CRC_AT: u64 = 96;

mod parts;
mod runs;

use parts::{align8, name_key, write_tail, Extensions, Initials};
use runs::{flush_run, merge_runs, Key, RUN_ROWS};

pub(in crate::browse) struct BuildOutput {
    pub(in crate::browse) counts: Counts,
    pub(in crate::browse) unix_paths: bool,
}

pub(in crate::browse) fn build(base: &CompactIndex, out: &Path) -> io::Result<BuildOutput> {
    let len = base.len() as usize;
    if len > u32::MAX as usize {
        return Err(io::Error::other(
            "browse columns support at most u32::MAX records",
        ));
    }
    let file = crate::compact_build::open_private(out)?
        .into_inner()
        .map_err(|e| e.into_error())?;
    let runs_path = crate::compact::suffix_path(out, ".runs");
    // A build killed mid-merge leaves its run file behind; it is scratch data.
    match std::fs::remove_file(&runs_path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    let runs = crate::compact_build::open_private(&runs_path)?
        .into_inner()
        .map_err(|e| e.into_error())?;
    let result = build_into(base, &file, &runs, len);
    drop(runs);
    let _ = std::fs::remove_file(&runs_path);
    let output = result?;
    file.sync_all()?;
    Ok(output)
}

fn build_into(
    base: &CompactIndex,
    file: &File,
    runs: &File,
    len: usize,
) -> io::Result<BuildOutput> {
    let attrs_at = HEADER as u64;
    let size_at = attrs_at + 4 * len as u64;
    let mtime_at = align8(size_at + (SIZE_BYTES * len) as u64);
    let perm_at = align8(mtime_at + 4 * len as u64);
    let table_at = align8(perm_at + 4 * len as u64);
    let lists_at = table_at + (INITIAL_KEYS * INITIAL_ENTRY) as u64;
    let (mut attrs, mut size, mut mtime) = (
        Section::new(file, attrs_at),
        Section::new(file, size_at),
        Section::new(file, mtime_at),
    );
    let mut counts = Counts::default();
    let mut unix_paths = true;
    let mut exts = Extensions::default();
    let mut initials = Initials::new();
    let mut rows: Vec<(Key, u32)> = Vec::with_capacity(RUN_ROWS);
    let (mut written, mut run_ends) = (0u64, Vec::new());
    let mut next_id = 0u32;
    for block in 0..base.block_count() {
        for record in base.read_block(block as u32)?.iter() {
            let id = next_id;
            next_id += 1;
            let ext = exts.id(record);
            let stamp = clamp_mtime(record.mtime);
            attrs.push(&pack_attrs(record, ext).to_le_bytes())?;
            size.push(&record.disk_bytes().min(SIZE_MAX).to_le_bytes()[..SIZE_BYTES])?;
            mtime.push(&stamp.to_le_bytes())?;
            counts.add_name(record.name(), 1);
            unix_paths &= record.path.starts_with('/');
            initials.offer(record.name(), stamp, id);
            rows.push((name_key(record.name()), id));
            if rows.len() == RUN_ROWS {
                flush_run(&mut rows, runs, &mut written)?;
                run_ends.push(written);
            }
        }
        if block % 100_000 == 0 {
            eprintln!("Preparing browse columns: {} / {len} records", next_id);
        }
    }
    if !rows.is_empty() {
        flush_run(&mut rows, runs, &mut written)?;
        run_ends.push(written);
    }
    drop(rows);
    attrs.finish()?;
    size.finish()?;
    mtime.finish()?;
    eprintln!("Preparing browse name order");
    let mut perm = Section::new(file, perm_at);
    merge_runs(base, runs, &run_ends, &mut perm)?;
    perm.finish()?;
    let tail = write_tail(file, lists_at, table_at, &initials, &exts)?;
    let mut header = vec![0u8; HEADER];
    header[..8].copy_from_slice(MAGIC);
    let words = [
        base.generation(),
        len as u64,
        attrs_at,
        size_at,
        mtime_at,
        perm_at,
        table_at,
        tail.ext_at,
        exts.count() as u64,
        tail.total,
        u64::from(exts.overflowed()),
    ];
    for (index, word) in words.iter().enumerate() {
        header[8 + index * 8..16 + index * 8].copy_from_slice(&word.to_le_bytes());
    }
    write_all_at(file, &header, 0)?;
    let total = tail.total;
    let mut crc = crc32fast::Hasher::new();
    let mut buffer = vec![0u8; 1 << 20];
    let mut offset = 0u64;
    while offset < total {
        let take = (total - offset).min(buffer.len() as u64) as usize;
        crate::browse::columns::write::read_exact_at(file, &mut buffer[..take], offset)?;
        crc.update(&buffer[..take]);
        offset += take as u64;
    }
    write_all_at(file, &crc.finalize().to_le_bytes(), CRC_AT)?;
    Ok(BuildOutput { counts, unix_paths })
}
