//! Decoder for the entries of an NTFS `$ATTRIBUTE_LIST` value.

use crate::geometry::{u16le, u32le, u64le};

pub(super) struct AttributeListEntry {
    pub(super) attribute_kind: u32,
    pub(super) lowest_vcn: u64,
    pub(super) record: u64,
    pub(super) sequence: u16,
    pub(super) attribute_id: u16,
}

/// Decode the attribute-list value using each entry own record length.
/// Nameless entries are 32 bytes: kind@0, length@4, name len@6, name off@7,
/// lowest VCN@8, file reference@16 (low 48 bits segment, high 16 sequence),
/// attribute id@24. There is no instance field.
pub(super) fn parse_attribute_list_entries(value: &[u8]) -> Vec<AttributeListEntry> {
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
