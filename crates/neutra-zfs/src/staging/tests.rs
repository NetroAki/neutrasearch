use super::{anonymous_spool, capture, validate_and_emit};
use neutra_core::FileRecord;
use std::io::Cursor;
use std::path::Path;

fn stream(complete: bool) -> Vec<u8> {
    let mut bytes = b"NSZF0001".to_vec();
    bytes.extend_from_slice(&1u32.to_le_bytes());
    for value in [0u64, 512, 1, 0o040755, 3, 3] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.push(b'/');
    if complete {
        bytes.extend_from_slice(&0u32.to_le_bytes());
    }
    bytes
}

fn stage(bytes: &[u8], child_success: bool, emitted: &mut Vec<FileRecord>) -> anyhow::Result<()> {
    let mut spool = Cursor::new(Vec::new());
    capture(Cursor::new(bytes), &mut spool)?;
    validate_and_emit(&mut spool, Path::new("/qa"), child_success, &mut |record| {
        emitted.push(record)
    })?;
    Ok(())
}

#[test]
fn child_failure_discards_a_valid_looking_stream() {
    let mut emitted = Vec::new();
    assert!(stage(&stream(true), false, &mut emitted).is_err());
    assert!(emitted.is_empty());
}

#[test]
fn truncated_stream_emits_no_records() {
    let mut emitted = Vec::new();
    assert!(stage(&stream(false), true, &mut emitted).is_err());
    assert!(emitted.is_empty());
}

#[test]
fn valid_stream_emits_all_records_after_validation() {
    let mut emitted = Vec::new();
    stage(&stream(true), true, &mut emitted).unwrap();
    assert_eq!(emitted.len(), 1);
    assert_eq!(emitted[0].path.as_ref(), "/qa");
}

#[test]
#[ignore = "requires writable disk-backed /var/tmp; run separately on the native test host"]
fn anonymous_spool_uses_the_native_disk_staging_path() {
    let mut spool = anonymous_spool().unwrap();
    capture(Cursor::new(stream(true)), &mut spool).unwrap();
    let mut emitted = Vec::new();
    validate_and_emit(&mut spool, Path::new("/qa"), true, &mut |record| {
        emitted.push(record)
    })
    .unwrap();
    assert_eq!(emitted.len(), 1);
}
