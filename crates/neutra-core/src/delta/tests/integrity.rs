use super::*;

#[test]
fn complete_frame_with_bad_checksum_fails_closed() {
    let path =
        std::env::temp_dir().join(format!("neutra-delta-corrupt-{}.wal", std::process::id()));
    remove_log(&path);
    let mut delta = DeltaIndex::open(&path, 11).unwrap();
    delta.apply(DeltaChange::Upsert(record("/a", 1))).unwrap();
    delta.sync().unwrap();
    drop(delta);

    let mut bytes = std::fs::read(&path).unwrap();
    *bytes.last_mut().unwrap() ^= 0x80;
    std::fs::write(&path, bytes).unwrap();
    assert!(DeltaIndex::open(&path, 11).is_err());
    remove_log(&path);
}

#[cfg(unix)]
#[test]
fn same_length_wal_rewrite_invalidates_the_persistent_cursor_and_checks_crc() {
    let path =
        std::env::temp_dir().join(format!("neutra-delta-rewrite-{}.wal", std::process::id()));
    remove_log(&path);
    {
        let mut delta = DeltaIndex::open(&path, 15).unwrap();
        delta.apply(DeltaChange::Upsert(record("/a", 1))).unwrap();
    }
    let mut bytes = std::fs::read(&path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0x80;
    std::fs::write(&path, bytes).unwrap();
    assert!(DeltaIndex::open(&path, 15).is_err());
    remove_log(&path);
    let overlay = path.parent().unwrap().join(".delta.overlay");
    let db = overlay.join(format!(
        "{}.resolved.sqlite",
        path.file_name().unwrap().to_string_lossy()
    ));
    let _ = std::fs::remove_file(db);
}

#[test]
fn torn_tail_is_truncated_before_new_appends() {
    let path = std::env::temp_dir().join(format!("neutra-delta-tail-{}.wal", std::process::id()));
    remove_log(&path);
    {
        let mut delta = DeltaIndex::open(&path, 9).unwrap();
        delta.apply(DeltaChange::Upsert(record("/a", 1))).unwrap();
        delta.sync().unwrap();
    }
    let verified_bytes = std::fs::metadata(&path).unwrap().len();
    let mut torn = OpenOptions::new().write(true).open(&path).unwrap();
    torn.seek(SeekFrom::End(0)).unwrap();
    torn.write_all(&[4, 0]).unwrap();
    drop(torn);
    {
        let mut recovered = DeltaIndex::open(&path, 9).unwrap();
        assert_eq!(recovered.wal_bytes(), verified_bytes);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), verified_bytes);
        recovered
            .apply(DeltaChange::Upsert(record("/b", 2)))
            .unwrap();
        recovered.sync().unwrap();
    }
    let reopened = DeltaIndex::open(&path, 9).unwrap();
    assert_eq!(reopened.change_count().unwrap(), 2);
    drop(reopened);
    remove_log(&path);
}
