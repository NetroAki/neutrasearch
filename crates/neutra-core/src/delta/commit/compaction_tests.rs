use super::*;
use crate::{FileKind, FsKind};

fn record(path: &str, size: u64) -> FileRecord {
    FileRecord {
        path: path.into(),
        size,
        disk: 0,
        mtime: 0,
        mode: 0,
        kind: FileKind::File,
        fs: FsKind::Btrfs,
        native_id: 0,
        native_parent: 0,
        source: 0,
    }
}

fn remove(path: &Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(lock_path(path));
}

fn temp_path(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("neutra-tail-{label}-{}.wal", std::process::id()))
}

#[test]
fn visits_only_the_ordered_tail_and_returns_its_cursor() {
    let path = temp_path("ordered");
    remove(&path);
    let mut delta = DeltaIndex::open(&path, 41).unwrap();
    delta.apply(DeltaChange::Upsert(record("/own", 1))).unwrap();
    let frozen = delta.compaction_cursor();
    let expected = vec![
        DeltaChange::RemovePrefix("/tree".into()),
        DeltaChange::Upsert(record("/tree/new", 2)),
        DeltaChange::Remove("/tree/old".into()),
    ];
    for change in expected.clone() {
        delta.apply(change).unwrap();
    }
    let mut actual = Vec::new();
    let end = delta
        .visit_changes_after(frozen, |change| {
            actual.push(change);
            Ok(())
        })
        .unwrap();
    assert_eq!(actual.len(), expected.len());
    let paths = actual
        .iter()
        .map(|change| match change {
            DeltaChange::Upsert(record) => record.path.to_string(),
            DeltaChange::Remove(path) | DeltaChange::RemovePrefix(path) => path.to_string(),
        })
        .collect::<Vec<_>>();
    assert_eq!(paths, ["/tree", "/tree/new", "/tree/old"]);
    assert_eq!(end.bytes, delta.compaction_cursor().bytes);
    assert!(delta
        .visit_changes_after(end, |_| panic!("end cursor must have an empty tail"))
        .is_ok());
    drop(delta);
    remove(&path);
}

#[test]
fn reset_generation_makes_old_cursor_retryable() {
    let path = temp_path("reset");
    remove(&path);
    let mut delta = DeltaIndex::open(&path, 51).unwrap();
    let frozen = delta.compaction_cursor();
    delta.reset(52).unwrap();
    let error = delta.visit_changes_after(frozen, |_| Ok(())).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
    drop(delta);
    remove(&path);
}

#[test]
fn corrupt_or_truncated_tail_fails_closed() {
    for truncate in [false, true] {
        let path = temp_path(if truncate { "truncated" } else { "crc" });
        remove(&path);
        let mut delta = DeltaIndex::open(&path, 61).unwrap();
        let frozen = delta.compaction_cursor();
        delta
            .apply(DeltaChange::Upsert(record("/tail", 3)))
            .unwrap();
        let end = delta.wal_bytes();
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        if truncate {
            file.set_len(end - 1).unwrap();
        } else {
            file.seek(SeekFrom::Start(end - 1)).unwrap();
            let mut byte = [0u8; 1];
            file.read_exact(&mut byte).unwrap();
            byte[0] ^= 0x80;
            file.seek(SeekFrom::Start(end - 1)).unwrap();
            file.write_all(&byte).unwrap();
        }
        drop(file);
        let error = delta.visit_changes_after(frozen, |_| Ok(())).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        drop(delta);
        remove(&path);
    }
}
