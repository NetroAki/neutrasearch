//! Bounds and integrity checks for the memory-mapped column format.
use super::{INITIAL_DEPTH, INITIAL_ENTRY, INITIAL_KEYS, MAGIC, OTHER_EXT, SIZE_BYTES};
use memmap2::Mmap;
use std::{io, ops::Range};

const HEADER: usize = 128;
const CRC_AT: usize = 96;

pub(super) struct Validated {
    pub(super) len: usize,
    pub(super) attrs: usize,
    pub(super) size: usize,
    pub(super) mtime: usize,
    pub(super) perm: usize,
    pub(super) initial_table: usize,
    pub(super) exts: Vec<Box<str>>,
    pub(super) ext_overflow: bool,
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn section(start: usize, count: usize, width: usize, end: usize) -> io::Result<Range<usize>> {
    let bytes = count
        .checked_mul(width)
        .ok_or_else(|| invalid("browse column size overflow"))?;
    let stop = start
        .checked_add(bytes)
        .ok_or_else(|| invalid("browse column offset overflow"))?;
    if stop > end {
        return Err(invalid("browse column section is truncated"));
    }
    Ok(start..stop)
}

fn word(map: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(map[at..at + 8].try_into().expect("validated header"))
}

pub(super) fn validate(map: &Mmap, generation: u64, records: u64) -> io::Result<Validated> {
    let bytes: &[u8] = map;
    if bytes.len() < HEADER || &bytes[..8] != MAGIC {
        return Err(invalid("unknown browse columns format"));
    }
    if word(bytes, 8) != generation || word(bytes, 16) != records {
        return Err(invalid("browse columns belong to another base"));
    }
    let len = usize::try_from(records).map_err(|_| invalid("too many browse records"))?;
    let end =
        usize::try_from(word(bytes, 80)).map_err(|_| invalid("browse columns are truncated"))?;
    if end != bytes.len() {
        return Err(invalid("browse columns length mismatch"));
    }
    let attrs = HEADER;
    let attrs_range = section(attrs, len, 4, end)?;
    let size = attrs_range.end;
    let size_range = section(size, len, SIZE_BYTES, end)?;
    let mtime = align(size_range.end);
    let mtime_range = section(mtime, len, 4, end)?;
    let perm = align(mtime_range.end);
    let perm_range = section(perm, len, 4, end)?;
    let table = align(perm_range.end);
    let table_range = section(table, INITIAL_KEYS, INITIAL_ENTRY, end)?;
    if [
        word(bytes, 24),
        word(bytes, 32),
        word(bytes, 40),
        word(bytes, 48),
        word(bytes, 56),
    ] != [
        attrs as u64,
        size as u64,
        mtime as u64,
        perm as u64,
        table as u64,
    ] {
        return Err(invalid("invalid browse column offsets"));
    }
    let ext_count =
        usize::try_from(word(bytes, 72)).map_err(|_| invalid("invalid extension count"))?;
    if ext_count > usize::from(OTHER_EXT) {
        return Err(invalid("invalid extension count"));
    }
    let ext_at =
        usize::try_from(word(bytes, 64)).map_err(|_| invalid("invalid extension offset"))?;
    let total = usize::try_from(word(bytes, 80)).map_err(|_| invalid("invalid column length"))?;
    if ext_at < table_range.end || ext_at > total {
        return Err(invalid("invalid extension offset"));
    }
    let mut cursor = table_range.end;
    for slot in 0..INITIAL_KEYS {
        let entry = table + slot * INITIAL_ENTRY;
        let offset =
            usize::try_from(word(bytes, entry)).map_err(|_| invalid("invalid initial offset"))?;
        let count = u32::from_le_bytes(bytes[entry + 8..entry + 12].try_into().unwrap()) as usize;
        let list = section(offset, count, 4, ext_at)?;
        if count > INITIAL_DEPTH || offset != cursor {
            return Err(invalid("invalid initial list bounds"));
        }
        for id in bytes[list.clone()].chunks_exact(4) {
            if u32::from_le_bytes(id.try_into().unwrap()) as usize >= len {
                return Err(invalid("invalid initial list record id"));
            }
        }
        cursor = list.end;
    }
    if cursor > ext_at {
        return Err(invalid("overlapping initial lists and extensions"));
    }
    let mut exts = Vec::new();
    cursor = ext_at;
    for _ in 0..ext_count {
        let size = *bytes
            .get(cursor)
            .ok_or_else(|| invalid("truncated extension table"))? as usize;
        let next = cursor
            .checked_add(1 + size)
            .ok_or_else(|| invalid("extension offset overflow"))?;
        let raw = bytes
            .get(cursor + 1..next)
            .ok_or_else(|| invalid("truncated extension table"))?;
        let ext = std::str::from_utf8(raw).map_err(|_| invalid("invalid extension encoding"))?;
        exts.push(ext.into());
        cursor = next;
    }
    if cursor != total {
        return Err(invalid("extension table length mismatch"));
    }
    if len > u32::MAX as usize {
        return Err(invalid("too many browse records"));
    }
    let mut seen = vec![0u8; len.div_ceil(8)];
    for raw in bytes[perm_range].chunks_exact(4) {
        let id = u32::from_le_bytes(raw.try_into().unwrap()) as usize;
        if id >= len || seen[id / 8] & (1 << (id % 8)) != 0 {
            return Err(invalid("invalid name-order record ids"));
        }
        seen[id / 8] |= 1 << (id % 8);
    }
    let mut crc = crc32fast::Hasher::new();
    crc.update(&bytes[..CRC_AT]);
    crc.update(&[0; 4]);
    crc.update(&bytes[CRC_AT + 4..]);
    if crc.finalize() != u32::from_le_bytes(bytes[CRC_AT..CRC_AT + 4].try_into().unwrap()) {
        return Err(invalid("browse columns checksum mismatch"));
    }
    Ok(Validated {
        len,
        attrs,
        size,
        mtime,
        perm,
        initial_table: table,
        exts,
        ext_overflow: word(bytes, 88) != 0,
    })
}

fn align(value: usize) -> usize {
    value.div_ceil(8) * 8
}
