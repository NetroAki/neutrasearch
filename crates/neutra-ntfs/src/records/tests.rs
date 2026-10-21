use super::*;

fn record_with_name(words: &[u16]) -> Vec<u8> {
    let mut rec = vec![0u8; 256];
    rec[20..22].copy_from_slice(&48u16.to_le_bytes());
    rec[48..52].copy_from_slice(&0x30u32.to_le_bytes());
    let attr_len = 24 + 66 + words.len() * 2;
    rec[52..56].copy_from_slice(&(attr_len as u32).to_le_bytes());
    rec[56] = 0;
    rec[64..68].copy_from_slice(&((66 + words.len() * 2) as u32).to_le_bytes());
    rec[68..70].copy_from_slice(&24u16.to_le_bytes());
    let value = &mut rec[72..];
    value[0..8].copy_from_slice(&5u64.to_le_bytes());
    value[64] = words.len() as u8;
    value[65] = 1;
    for (i, word) in words.iter().enumerate() {
        value[66 + i * 2..68 + i * 2].copy_from_slice(&word.to_le_bytes());
    }
    rec[48 + attr_len..52 + attr_len].copy_from_slice(&0xffff_ffffu32.to_le_bytes());
    rec
}

#[test]
fn filename_decoder_rejects_lone_surrogates_and_keeps_literal_replacement_character() {
    assert!(parse_record(&record_with_name(&[0xd800]), false).is_err());
    let (entry, _, _) = parse_record(&record_with_name(&[0xfffd]), false)
        .unwrap()
        .unwrap();
    assert_eq!(entry.name, "\u{fffd}");
}

#[test]
fn allocated_integrity_errors_fail_but_unused_records_skip() {
    let mut record = vec![0; 1024];
    record[..4].copy_from_slice(b"FILE");
    assert!(prepare_allocated(&mut record, 512).unwrap().is_none());
    record[22..24].copy_from_slice(&1u16.to_le_bytes());
    assert!(prepare_allocated(&mut record, 512).is_err());
}

#[test]
fn malformed_allocated_attributes_fail_instead_of_ending_early() {
    let mut record = record_with_name(&[b'a' as u16]);
    record[52..56].copy_from_slice(&0u32.to_le_bytes());
    assert!(parse_record(&record, false).is_err());
    let mut record = record_with_name(&[b'a' as u16]);
    record[64..68].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(parse_record(&record, false).is_err());
}
