use super::*;
use std::os::unix::ffi::OsStrExt;

fn mount() -> MountInfo {
    MountInfo {
        device: "test".into(),
        mountpoint: PathBuf::from("/"),
        fs: neutra_core::FsKind::Ext4,
        source: neutra_core::MountSource::Local,
    }
}

#[test]
fn remove_and_upsert_reject_invalid_bytes_and_preserve_replacement_character() {
    let invalid = PathBuf::from(std::ffi::OsStr::from_bytes(b"/bad\xffname"));
    let replacement = PathBuf::from("/bad�name");
    let mut changes = BTreeMap::new();
    assert_eq!(
        insert_remove(&mut changes, &invalid).unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
    assert!(make_record(&invalid, &zeroed_stat(), 1, &mount(), 1).is_err());

    insert_remove(&mut changes, &replacement).unwrap();
    assert!(matches!(
        changes.get("/bad�name"),
        Some(DeltaChange::Remove(_))
    ));
    assert!(insert_remove(&mut changes, &invalid).is_err());
    assert_eq!(changes.len(), 1);
    changes.clear();
    let record = make_record(&replacement, &zeroed_stat(), 1, &mount(), 1).unwrap();
    insert_upsert(&mut changes, record);
    assert!(matches!(
        changes.get("/bad�name"),
        Some(DeltaChange::Upsert(_))
    ));
}
