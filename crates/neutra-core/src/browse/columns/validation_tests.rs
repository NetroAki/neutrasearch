use super::*;
use crate::{CompactIndex, FileKind, FileRecord, FsKind};

#[test]
fn damaged_columns_fail_before_lookup_and_can_be_rebuilt() {
    let root = std::env::temp_dir().join(format!("ns-columns-integrity-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let index = root.join("base.nsx");
    let records: Vec<_> = (0..3)
        .map(|id| FileRecord {
            path: format!("/files/alpha{id}.txt").into(),
            size: 10,
            disk: 10,
            mtime: 1,
            mode: 0,
            kind: FileKind::File,
            fs: FsKind::Btrfs,
            native_id: id,
            native_parent: 0,
            source: 0,
        })
        .collect();
    CompactIndex::build(&records, &index).unwrap();
    let base = CompactIndex::open_fast(&index).unwrap();
    let columns = Columns::path_for(&crate::BrowserIndex::path_for(&index));
    crate::BrowserIndex::ensure(&index, base.generation()).unwrap();
    let original = std::fs::read(&columns).unwrap();
    let valid = Columns::open(&columns, base.generation(), base.len()).unwrap();
    assert_eq!(valid.len(), 3);
    assert_eq!(valid.attrs(0) & 0xffff, 0);
    drop(valid);
    let word = |at| u64::from_le_bytes(original[at..at + 8].try_into().unwrap()) as usize;
    for (at, value, width) in [
        (24, original.len() as u64, 8),
        (32, u64::MAX, 8),
        (40, u64::MAX, 8),
        (48, u64::MAX, 8),
        (56, u64::MAX, 8),
        (64, u64::MAX, 8),
        (72, u64::MAX, 8),
        (word(56), u64::MAX, 8),
        (word(56) + 8, u32::MAX as u64, 4),
        (word(48), u32::MAX as u64, 4),
        (word(word(56) + 97 * INITIAL_ENTRY), u32::MAX as u64, 4),
        (word(24), 123, 4),
    ] {
        let mut damaged = original.clone();
        damaged[at..at + width].copy_from_slice(&value.to_le_bytes()[..width]);
        std::fs::write(&columns, damaged).unwrap();
        assert!(
            Columns::open(&columns, base.generation(), base.len()).is_err(),
            "offset {at}"
        );
    }
    crate::BrowserIndex::ensure(&index, base.generation()).unwrap();
    assert!(Columns::open(&columns, base.generation(), base.len()).is_ok());
    drop(base);
    std::fs::remove_dir_all(root).unwrap();
}
