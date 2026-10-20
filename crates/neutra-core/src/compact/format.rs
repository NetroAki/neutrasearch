use super::CompactIndex;
mod layout;
pub(super) use layout::IndexBytes;
pub(crate) use layout::{
    BlockDesc, DictEntry, BLOCK_RECORDS, CHECKSUM_BYTES, DESC_SIZE, DICT_SIZE, HEADER, MAGIC,
    MIN_READABLE_VERSION, VERSION,
};
mod codec;
pub(crate) use codec::{binerr, invalid, put_varint};
pub(super) use codec::{checked, get_varint, u16_at, u32_at, u64_at};
mod table;
pub(super) use table::TableView;
