#[cfg(not(windows))]
use memmap2::Mmap;

pub(crate) const MAGIC: &[u8; 8] = b"NEUTIDX1";
pub(crate) const VERSION: u32 = 5;
/// Newest readable record layout. Bases at this version store disk bytes;
/// version 3 bases remain readable with disk falling back to size.
pub(crate) const MIN_READABLE_VERSION: u32 = 3;
pub(crate) const HEADER: u64 = 64;
pub(crate) const CHECKSUM_BYTES: usize = 4;
pub(crate) const BLOCK_RECORDS: usize = 32;
pub(crate) const DESC_SIZE: u64 = 16;
pub(crate) const DICT_SIZE: u64 = 16;

#[derive(Clone, Copy)]
pub(crate) struct BlockDesc {
    pub(crate) offset: u64,
    pub(crate) len: u32,
    pub(crate) count: u16,
}
#[derive(Clone, Copy)]
pub(crate) struct DictEntry {
    pub(crate) gram: u32,
    pub(crate) len: u32,
    pub(crate) offset: u64,
}

#[cfg(not(windows))]
pub(in crate::compact) type IndexBytes = Mmap;
#[cfg(windows)]
pub(in crate::compact) type IndexBytes = Vec<u8>;
