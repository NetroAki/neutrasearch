use super::*;
use crate::records::append_component;
#[test]
fn root_parent_sequence_uses_the_preseeded_root_path() {
    let entries = HashMap::from([(
        6,
        Entry {
            parent: 5,
            parent_sequence: 42,
            sequence: 1,
            name: "Windows".into(),
            size: 0,
            alloc: 0,
            mtime: 0,
            dir: true,
        },
    )]);
    let mut cache = HashMap::from([(5, String::new())]);
    let mut seen = std::collections::HashSet::new();
    assert!(ensure_dir_path(5, 42, 42, &entries, &mut cache, &mut seen).is_ok());
    assert!(ensure_dir_path(5, 7, 42, &entries, &mut cache, &mut seen).is_err());
    assert_eq!(cache.get(&5).map(String::as_str), Some(""));
}

#[test]
fn path_cycles_stop() {
    let mut e = HashMap::new();
    e.insert(
        6,
        Entry {
            parent: 5,
            parent_sequence: 0,
            sequence: 1,
            name: "a".into(),
            size: 0,
            alloc: 0,
            mtime: 0,
            dir: true,
        },
    );
    e.insert(
        7,
        Entry {
            parent: 6,
            parent_sequence: 1,
            sequence: 1,
            name: "b".into(),
            size: 0,
            alloc: 0,
            mtime: 0,
            dir: false,
        },
    );
    let mut c = HashMap::from([(5, String::new())]);
    let mut seen = std::collections::HashSet::new();
    assert!(ensure_dir_path(6, 1, 1, &e, &mut c, &mut seen).is_ok());
    assert_eq!(append_component(c.get(&6).unwrap(), "b"), "a/b");
    e.get_mut(&6).unwrap().parent = 7;
    let mut c = HashMap::from([(5, String::new())]);
    assert!(ensure_dir_path(6, 1, 1, &e, &mut c, &mut seen).is_err());
}

#[test]
fn scan_hard_fails_when_the_volume_will_not_open() {
    // Never walk as a fallback: an unreadable volume is a setup bug,
    // not a slow path.
    let mount = MountInfo {
        device: "/nonexistent-volume".into(),
        mountpoint: std::env::temp_dir(),
        fs: FsKind::Ntfs,
        source: neutra_core::MountSource::Local,
    };
    let mut records = Vec::new();
    let error = scan(&mount, &mut |record| records.push(record))
        .unwrap_err()
        .to_string();
    assert!(error.contains("privileges"), "unexpected: {error}");
    assert!(records.is_empty());
}

fn scan_image(image: Vec<u8>) -> anyhow::Result<Vec<FileRecord>> {
    let size = image.len() as u64;
    let mut records = Vec::new();
    scan_reader(std::io::Cursor::new(image), size, "/vol", &mut |r| {
        records.push(r)
    })?;
    Ok(records)
}

#[test]
fn attribute_list_image_scans_when_the_size_is_honest() {
    let records = scan_image(test_images::attr_list_image(32)).unwrap();
    assert!(records.iter().any(|r| r.path.ends_with("docs/a.txt")));
    assert!(records.iter().any(|r| r.path.ends_with("docs/b.txt")));
}

#[test]
fn forged_attribute_list_size_is_an_error_not_an_allocation_abort() {
    let error = scan_image(test_images::attr_list_image(36_000_000_000_000_000)).unwrap_err();
    let error = format!("{error:#}");
    assert!(
        error.contains("implausible attribute list"),
        "unexpected: {error}"
    );
}

#[test]
fn forged_mft_run_longer_than_the_device_is_rejected() {
    let image = test_images::declared_huge_mft_image();
    assert!(image.len() < 16 * 1024);
    let error = format!("{:#}", scan_image(image).unwrap_err());
    assert!(error.contains("past the"), "unexpected: {error}");
}

fn scan_image_fails_without_records(kind: &str) -> String {
    let image = test_images::unresolved_parent_image(kind);
    let size = image.len() as u64;
    let mut published = 0u32;
    let error = scan_reader(std::io::Cursor::new(image), size, "/vol", &mut |_| {
        published += 1;
    })
    .unwrap_err();
    assert_eq!(published, 0, "torn branch published records before failing");
    format!("{error:#}")
}

#[test]
fn unresolved_missing_parent_fails_before_publication() {
    let error = scan_image_fails_without_records("missing");
    assert!(error.contains("missing"), "unexpected: {error}");
}

#[test]
fn unresolved_parent_sequence_mismatch_fails_before_publication() {
    let error = scan_image_fails_without_records("sequence");
    assert!(error.contains("sequence mismatch"), "unexpected: {error}");
}

#[test]
fn unresolved_parent_cycle_fails_before_publication() {
    let error = scan_image_fails_without_records("cycle");
    assert!(error.contains("cycle"), "unexpected: {error}");
}

#[test]
fn unresolved_root_sequence_mismatch_fails_before_publication() {
    let error = scan_image_fails_without_records("root-sequence");
    assert!(error.contains("record 5"), "unexpected: {error}");
}

#[test]
fn unresolved_alias_parent_fails_before_publication() {
    let error = scan_image_fails_without_records("alias-missing");
    assert!(error.contains("alias"), "unexpected: {error}");
}

#[test]
fn unresolved_alias_sequence_mismatch_fails_before_publication() {
    let error = scan_image_fails_without_records("alias-sequence");
    assert!(error.contains("sequence mismatch"), "unexpected: {error}");
}
