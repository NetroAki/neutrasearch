//! Immutable sort columns for one pinned catalog base.
//!
//! The catalog used to keep eight B-tree indexes, each repeating the full path
//! and name of every record. These flat, memory-mapped columns hold only what
//! ordering and filtering need (about 17 bytes per record); full records stay
//! in the compressed base and are decoded on demand. Live changes sit in the
//! small SQLite side of the catalog and are merged by the query layer.

mod build;
mod lookup;
mod prefilter;
mod scan;
mod validation;
#[cfg(test)]
mod validation_tests;
mod write;

pub(super) use build::build;
pub(super) use lookup::BlockReader;
pub(super) use prefilter::Prefilter;
pub(super) use scan::{Boundary, Order, Span};

use crate::FileRecord;
use memmap2::Mmap;
use std::{fs::File, io, path::Path};

const MAGIC: &[u8; 8] = b"NSCOL004";
const HEADER: usize = 128;
pub(super) const INITIAL_DEPTH: usize = 4097;
pub(super) const OTHER_EXT: u16 = u16::MAX;
/// 128 ASCII initials (letters folded) plus the two non-ASCII capitals that
/// fold onto "i" and "k" in Unicode but not in SQLite NOCASE.
pub(super) const INITIAL_KEYS: usize = 130;
const INITIAL_ENTRY: usize = 16;

pub(super) struct Columns {
    map: Mmap,
    len: usize,
    attrs: usize,
    size: usize,
    mtime: usize,
    perm: usize,
    initial_table: usize,
    exts: Vec<Box<str>>,
    ext_overflow: bool,
}

pub(super) fn fs_code(fs: &crate::FsKind) -> u32 {
    match fs {
        crate::FsKind::Btrfs => 0,
        crate::FsKind::Ext4 => 1,
        crate::FsKind::Ntfs => 2,
        crate::FsKind::Zfs => 3,
        _ => 15,
    }
}

pub(super) fn kind_code(kind: crate::FileKind) -> u32 {
    match kind {
        crate::FileKind::File => 0,
        crate::FileKind::Dir => 1,
        crate::FileKind::Symlink => 2,
        crate::FileKind::Other => 3,
    }
}

pub(super) fn pack_attrs(record: &FileRecord, ext: u16) -> u32 {
    u32::from(ext)
        | fs_code(&record.fs) << 16
        | kind_code(record.kind) << 20
        | u32::from(crate::query::is_executable(record)) << 23
}

pub(super) fn clamp_mtime(mtime: i64) -> u32 {
    if mtime > 4_102_444_800 {
        0
    } else {
        mtime.clamp(0, i64::from(u32::MAX)) as u32
    }
}

pub(super) const SIZE_BYTES: usize = 6;
pub(super) const SIZE_MAX: u64 = (1 << 48) - 1;

/// Initial-letter slot used by the recall lists, or None for other characters.
pub(super) fn initial_slot(first: char) -> Option<usize> {
    match first {
        '\u{130}' => Some(128),
        '\u{212A}' => Some(129),
        c if c.is_ascii() => Some(c.to_ascii_lowercase() as usize),
        _ => None,
    }
}

impl Columns {
    pub(super) fn path_for(catalog: &Path) -> std::path::PathBuf {
        let mut path = catalog.as_os_str().to_os_string();
        path.push(".cols");
        path.into()
    }

    pub(super) fn open(path: &Path, base_generation: u64, records: u64) -> io::Result<Self> {
        let file = File::open(path)?;
        // SAFETY: the file is private to the index owner and never modified
        // after publication; replacement is by atomic rename of a new file.
        let map = unsafe { Mmap::map(&file)? };
        let checked = validation::validate(&map, base_generation, records)?;
        Ok(Self {
            len: checked.len,
            attrs: checked.attrs,
            size: checked.size,
            mtime: checked.mtime,
            perm: checked.perm,
            initial_table: checked.initial_table,
            ext_overflow: checked.ext_overflow,
            exts: checked.exts,
            map,
        })
    }

    pub(super) fn len(&self) -> usize {
        self.len
    }

    fn word32(&self, at: usize) -> u32 {
        u32::from_le_bytes(self.map[at..at + 4].try_into().unwrap())
    }

    pub(super) fn attrs(&self, id: u32) -> u32 {
        self.word32(self.attrs + id as usize * 4)
    }

    pub(super) fn mtime(&self, id: u32) -> u32 {
        self.word32(self.mtime + id as usize * 4)
    }

    pub(super) fn size(&self, id: u32) -> u64 {
        let at = self.size + id as usize * SIZE_BYTES;
        let mut bytes = [0u8; 8];
        bytes[..SIZE_BYTES].copy_from_slice(&self.map[at..at + SIZE_BYTES]);
        u64::from_le_bytes(bytes)
    }

    /// Record id at a position in case-insensitive name order.
    pub(super) fn perm(&self, position: usize) -> u32 {
        self.word32(self.perm + position * 4)
    }

    pub(super) fn ext_id(&self, ext: &str) -> Option<u16> {
        self.exts
            .iter()
            .position(|known| known.as_ref() == ext)
            .map(|i| i as u16)
    }

    /// Most recently modified ids beginning with this initial, newest first.
    pub(super) fn initial_list(&self, slot: usize) -> Vec<u32> {
        let entry = self.initial_table + slot * INITIAL_ENTRY;
        let offset = u64::from_le_bytes(self.map[entry..entry + 8].try_into().unwrap()) as usize;
        let count = self.word32(entry + 8) as usize;
        (0..count).map(|i| self.word32(offset + i * 4)).collect()
    }
}
