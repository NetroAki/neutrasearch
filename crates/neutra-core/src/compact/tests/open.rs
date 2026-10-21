use super::test_support::rec;
use super::*;

#[test]
fn fast_reader_rejects_changed_block_contents() {
    let path = std::env::temp_dir().join(format!(
        "neutra-block-corruption-{}.nsx",
        std::process::id()
    ));
    CompactIndex::build(&[rec("/safe.txt", 10)], &path).unwrap();
    let index = CompactIndex::open_fast(&path).unwrap();
    let descriptor = index.block(0).unwrap();
    drop(index);
    let mut bytes = std::fs::read(&path).unwrap();
    let last = descriptor.offset as usize + descriptor.len as usize - 1;
    bytes[last] ^= 1;
    std::fs::write(&path, &bytes).unwrap();
    let index = CompactIndex::open_fast(&path).unwrap();
    assert!(index.read_block(0).is_err());
    drop(index);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn fast_reader_validates_legacy_payload_checksum() {
    let path = std::env::temp_dir().join(format!(
        "neutra-legacy-corruption-{}.nsx",
        std::process::id()
    ));
    CompactIndex::build(&[rec("/legacy.txt", 10)], &path).unwrap();
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[8..12].copy_from_slice(&5u32.to_le_bytes());
    assert!(CompactIndex::open_fast(&path).is_ok());
    std::fs::write(&path, &bytes).unwrap();
    assert!(CompactIndex::open_fast(&path).is_err());
    let end = bytes.len() - CHECKSUM_BYTES;
    let checksum = crc32fast::hash(&bytes[..end]);
    bytes[end..].copy_from_slice(&checksum.to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();
    let index = CompactIndex::open_fast(&path).unwrap();
    assert_eq!(index.read_block(0).unwrap()[0].path.as_ref(), "/legacy.txt");
    drop(index);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn fast_reader_rejects_unchecksummed_current_version_blocks() {
    let path = std::env::temp_dir().join(format!(
        "neutra-missing-block-checksum-{}.nsx",
        std::process::id()
    ));
    CompactIndex::build(&[rec("/safe.txt", 10)], &path).unwrap();
    let index = CompactIndex::open_fast(&path).unwrap();
    let offset = index.block(0).unwrap().offset as usize;
    drop(index);
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[offset + 4] &= !4;
    std::fs::write(&path, bytes).unwrap();
    let index = CompactIndex::open_fast(&path).unwrap();
    assert!(index.read_block(0).is_err());
    drop(index);
    std::fs::remove_file(path).unwrap();
}
#[test]
fn open_rejects_unknown_record_versions() {
    let path =
        std::env::temp_dir().join(format!("neutra-compact-badver-{}.nsx", std::process::id()));
    let mut bytes = vec![0u8; HEADER as usize + CHECKSUM_BYTES];
    bytes[..8].copy_from_slice(MAGIC);
    bytes[8..12].copy_from_slice(&99u32.to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();
    let error = match CompactIndex::open_fast(&path) {
        Ok(_) => panic!("unknown record version opened without error"),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("unsupported compact index version"),
        "unexpected error: {error}"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn fast_reader_verifies_header_dictionary_and_posting_routing() {
    let path = std::env::temp_dir().join(format!("neutra-routing-{}.nsx", std::process::id()));
    CompactIndex::build(&[rec("/alphabet.txt", 1)], &path).unwrap();
    let original = std::fs::read(&path).unwrap();
    let dict = u64::from_le_bytes(original[48..56].try_into().unwrap()) as usize;
    let postings = u64::from_le_bytes(original[40..48].try_into().unwrap()) as usize;
    for offset in [28, dict, postings] {
        let mut damaged = original.clone();
        damaged[offset] ^= 1;
        std::fs::write(&path, damaged).unwrap();
        assert!(
            CompactIndex::open_fast(&path).is_err(),
            "routing byte {offset}"
        );
    }
    std::fs::write(&path, original).unwrap();
    assert!(
        CompactIndex::open_fast(&path)
            .unwrap()
            .search(&Query::parse("alpha"))
            .unwrap()
            .0
            .len()
            == 1
    );
    std::fs::remove_file(path).unwrap();
}

mod listing;
