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
        0x80, 0x00, 0x00, 0x00, 0x20, 0x00, 0x00, 0x1a, 0x52, 0x1e, 0x34, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x08, 0x01, 0x00, 0x00, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00,
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

#[test]
fn malformed_attribute_list_run_offsets_fail_without_panicking() {
    for offset in [0u16, 63, 1000] {
        let (mut disk, mut rec0, base) = fixture(2);
        rec0[88..90].copy_from_slice(&offset.to_le_bytes());
        assert!(complete_mft_runs(&mut disk, &rec0, 4096, 1024, 512, base).is_err());
    }
}

#[test]
fn malformed_extension_run_offsets_fail_without_panicking() {
    for offset in [0u16, 63, 1000] {
        let (mut disk, rec0, base) = fixture(2);
        let start = 10 * 4096 + 5 * 1024 + 56 + 32;
        disk.get_mut()[start..start + 2].copy_from_slice(&offset.to_le_bytes());
        assert!(complete_mft_runs(&mut disk, &rec0, 4096, 1024, 512, base).is_err());
    }
}
