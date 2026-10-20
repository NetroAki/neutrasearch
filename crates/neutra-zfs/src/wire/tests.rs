use super::read;
use neutra_core::{FileKind, FileRecord};
use std::path::Path;

fn record(output: &mut Vec<u8>, path: &[u8], values: [u64; 6]) {
    output.extend_from_slice(&(path.len() as u32).to_le_bytes());
    for value in values {
        output.extend_from_slice(&value.to_le_bytes());
    }
    output.extend_from_slice(path);
}

fn stream() -> Vec<u8> {
    let mut output = b"NSZF0001".to_vec();
    record(&mut output, b"/", [0, 512, 1, 0o040755, 3, 3]);
    output
}

fn decode(bytes: &[u8]) -> anyhow::Result<Vec<FileRecord>> {
    let mut records = Vec::new();
    read(&mut &*bytes, Path::new("/qa"), &mut |record| {
        records.push(record)
    })?;
    Ok(records)
}

#[test]
fn sparse_sizes_links_and_names_survive_metadata_streaming() {
    let mut bytes = stream();
    record(
        &mut bytes,
        "/audio/calling-é\tstem.wav".as_bytes(),
        [2_199_023_255_552, 512, 42, 0o100640, 10, 3],
    );
    record(
        &mut bytes,
        b"/hardlink.wav",
        [2_199_023_255_552, 512, 42, 0o100640, 10, 3],
    );
    record(&mut bytes, b"/symlink", [3, 0, 43, 0o120777, 11, 3]);
    bytes.extend_from_slice(&0u32.to_le_bytes());
    let records = decode(&bytes).unwrap();
    assert_eq!(records[0].path.as_ref(), "/qa");
    assert_eq!(records[1].path.as_ref(), "/qa/audio/calling-é\tstem.wav");
    assert_eq!(records[1].size, 2_199_023_255_552);
    assert_eq!(records[1].disk_bytes(), 512);
    assert_eq!(records[1].native_id, records[2].native_id);
    assert_eq!(records[3].disk_bytes(), 0);
    assert_eq!(records[3].kind, FileKind::Symlink);
}

#[test]
fn malformed_or_incomplete_metadata_is_not_a_successful_scan() {
    let valid_root = stream();
    let mut finished = valid_root.clone();
    finished.extend_from_slice(&0u32.to_le_bytes());
    for length in 0..finished.len() {
        assert!(
            decode(&finished[..length]).is_err(),
            "accepted truncated length {length}"
        );
    }
    let mut trailing = finished.clone();
    trailing.push(1);
    assert!(decode(&trailing).is_err());
    let mut empty = b"NSZF0001".to_vec();
    empty.extend_from_slice(&0u32.to_le_bytes());
    assert!(decode(&empty).is_err());
    let mut oversized = valid_root.clone();
    oversized.extend_from_slice(&4097u32.to_le_bytes());
    assert!(decode(&oversized).is_err());
    for path in [
        &b"../escape"[..],
        b"/../escape",
        b"/a/./b",
        b"/a//b",
        b"/a\0b",
        b"/raw\xff",
    ] {
        let mut bytes = valid_root.clone();
        record(&mut bytes, path, [0, 0, 0, 0o100644, 4, 3]);
        bytes.extend_from_slice(&0u32.to_le_bytes());
        assert!(decode(&bytes).is_err(), "accepted {path:?}");
    }
}
