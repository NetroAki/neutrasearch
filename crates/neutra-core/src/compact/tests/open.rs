use super::test_support::{dir, rec};
use super::*;
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
fn list_directory_returns_direct_children_with_totals() {
    let mut records = vec![
        rec("/home/a/top.txt", 10),
        rec("/home/a/top.txt", 10),
        rec("/home/a/sub/deep.txt", 20),
        rec("/home/a/sub/deeper/x.txt", 5),
        dir("/home/a/empty"),
        rec("/home/b/other.txt", 7),
    ];
    // Apparent and on-disk sizes diverge: totals track both views.
    records[0].disk = 4;
    let path =
        std::env::temp_dir().join(format!("neutra-compact-{}-listing.idx", std::process::id()));
    CompactIndex::build(&records, &path).unwrap();
    let index = CompactIndex::open_fast(&path).unwrap();
    let listing = index.list_directory("/home/a", None, None).unwrap();
    // Duplicate alias pair counts once; outside paths stay out.
    assert_eq!(listing.total_size, 29);
    assert_eq!(listing.total_logical, 35);
    assert_eq!(listing.total_count, 3);
    assert_eq!(listing.files.len(), 1);
    assert_eq!(listing.files[0].name.as_ref(), "top.txt");
    assert_eq!(listing.files[0].size, 4);
    assert_eq!(listing.files[0].logical, 10);
    assert_eq!(listing.subdirs.len(), 2);
    assert_eq!(listing.subdirs[0].name.as_ref(), "sub");
    assert_eq!(listing.subdirs[0].size, 25);
    assert_eq!(listing.subdirs[0].files, 2);
    assert_eq!(listing.subdirs[1].name.as_ref(), "empty");
    let missing = index.list_directory("/nope", None, None).unwrap();
    assert_eq!(missing.total_count, 0);
    assert!(missing.files.is_empty() && missing.subdirs.is_empty());
    drop(index);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn list_directory_merges_live_delta_exactly() {
    let records = vec![rec("/d/f.txt", 10), rec("/d/sub/g.txt", 20)];
    let path = std::env::temp_dir().join(format!(
        "neutra-compact-{}-listing-delta.idx",
        std::process::id()
    ));
    CompactIndex::build(&records, &path).unwrap();
    let index = CompactIndex::open_fast(&path).unwrap();
    let wal = std::env::temp_dir().join(format!(
        "neutra-compact-{}-listing-delta.wal",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&wal);
    let mut delta = DeltaIndex::open(&wal, index.generation()).unwrap();
    delta
        .apply(crate::DeltaChange::Upsert(rec("/d/f.txt", 30)))
        .unwrap();
    delta
        .apply(crate::DeltaChange::Upsert(rec("/d/new.txt", 5)))
        .unwrap();
    delta
        .apply(crate::DeltaChange::Remove("/d/sub/g.txt".into()))
        .unwrap();
    delta.sync().unwrap();
    let listing = index.list_directory("/d", None, Some(&delta)).unwrap();
    assert_eq!(listing.total_size, 35);
    assert_eq!(listing.total_count, 2);
    assert_eq!(listing.files.len(), 2);
    assert_eq!(listing.files[0].name.as_ref(), "f.txt");
    assert_eq!(listing.files[0].size, 30);
    drop(index);
    drop(delta);
    std::fs::remove_file(path).unwrap();
    std::fs::remove_file(&wal).unwrap();
    let mut lock = wal.into_os_string();
    lock.push(".lock");
    let _ = std::fs::remove_file(lock);
}
