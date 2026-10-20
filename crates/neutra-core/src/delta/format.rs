pub(super) const MAGIC: &[u8; 8] = b"NEUTDLT4";
pub(super) const MAGIC_V3: &[u8; 8] = b"NEUTDLT3";
pub(super) const MAGIC_V2: &[u8; 8] = b"NEUTDLT2";
/// Magic written by binaries predating on-disk sizes. Logs under this magic
/// carry the frozen frame layout; replay selects the decoder from the file
/// magic, never by probing frames.
pub(super) const MAGIC_V1: &[u8; 8] = b"NEUTDLT1";
pub const DELTA_HEADER_BYTES: u64 = 16;
pub(super) const HEADER: u64 = DELTA_HEADER_BYTES;
pub(super) const MAX_FRAME: usize = 16 * 1024 * 1024;
pub const DEFAULT_COMPACT_AT: u64 = 64 * 1024 * 1024;

use std::io;
pub(super) fn codec(error: impl std::fmt::Display) -> io::Error {
    invalid(format!("delta codec: {error}"))
}
pub(super) fn invalid(error: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.into())
}
