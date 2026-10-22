use super::*;
use crate::tests::{build_test_base, record, remove_store, store_paths};

#[test]
fn hidden_filesystems_and_subdirectory_bind_mounts_cannot_be_repaired_as_whole_devices() {
    let root = "40 1 8:1 / /data rw - ext4 /dev/test rw\n";
    assert!(validate_layout(root, Path::new("/data")).is_ok());
    for fs in ["tmpfs", "overlay", "fuse.unknown"] {
        let raw = format!("{root}41 40 0:1 / /data/hidden rw - {fs} none rw\n");
        assert!(validate_layout(&raw, Path::new("/data")).is_err());
    }
    let bind = "40 1 8:1 /subdir /data rw - ext4 /dev/test rw\n";
    assert!(validate_layout(bind, Path::new("/data")).is_err());
    let spaced = "40 1 8:1 / /big\\040drive rw - ext4 /dev/test rw\n41 40 0:1 / /big\\040drive/hidden rw - tmpfs none rw\n";
    assert!(validate_layout(spaced, Path::new("/big drive")).is_err());
}

#[test]
fn failed_recovery_stays_blocked_and_retry_preserves_other_mounts() {
    let (path, wal) = store_paths("native-mount-repair");
    build_test_base(&[record("/drive/old", 1), record("/other/kept", 2)], &path).unwrap();
    let base = CompactIndex::open(&path).unwrap();
    let generation = base.generation();
    drop(base);
    BrowserIndex::ensure(&path, generation).unwrap();
    let marker = crate::store::stale_marker(&path);
    std::fs::write(&marker, "native event coverage lost").unwrap();
    let error = refresh(&path, Path::new("/drive"), |sink| {
        for n in 0..1024 {
            sink(record(&format!("/drive/partial-{n}"), n));
        }
        anyhow::bail!("injected native metadata failure")
    })
    .unwrap_err();
    assert!(error
        .to_string()
        .contains("injected native metadata failure"));
    assert!(marker.is_file());
    assert!(CompactIndex::open(&path).is_err());
    refresh(&path, Path::new("/drive"), |sink| {
        sink(record("/drive/current", 99));
        Ok(())
    })
    .unwrap();
    assert!(marker.is_file());
    assert!(CompactIndex::open(&path).is_err());
    let browser = BrowserIndex::open(&path, generation).unwrap();
    assert!(browser.record_by_path("/drive/old").unwrap().is_none());
    assert!(browser
        .record_by_path("/drive/partial-0")
        .unwrap()
        .is_none());
    assert_eq!(
        browser
            .record_by_path("/drive/current")
            .unwrap()
            .unwrap()
            .size,
        99
    );
    assert_eq!(
        browser.record_by_path("/other/kept").unwrap().unwrap().size,
        2
    );
    drop(browser);
    std::fs::remove_file(marker).unwrap();
    remove_store(&path, &wal);
}

#[test]
fn unexpected_native_path_cannot_publish_a_successful_recovery() {
    let (path, wal) = store_paths("native-mount-repair-boundary");
    build_test_base(&[record("/other/kept", 2)], &path).unwrap();
    let base = CompactIndex::open(&path).unwrap();
    BrowserIndex::ensure(&path, base.generation()).unwrap();
    drop(base);
    let marker = crate::store::stale_marker(&path);
    std::fs::write(&marker, "native event coverage lost").unwrap();
    assert!(refresh(&path, Path::new("/drive"), |sink| {
        sink(record("/other/bad", 1));
        Ok(())
    })
    .is_err());
    assert!(marker.is_file());
    std::fs::remove_file(marker).unwrap();
    remove_store(&path, &wal);
}
