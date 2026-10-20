#![cfg(target_os = "linux")]
use neutra_core::{DeltaChange, DeltaIndex};

#[test]
fn checkpoint_invalidates_a_frozen_compaction_cursor() {
    let root = std::env::temp_dir().join(format!("ns-compaction-cursor-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let path = root.join("base.delta");
    let mut writer = DeltaIndex::open(&path, 10).unwrap();
    writer.apply(DeltaChange::Remove("/before".into())).unwrap();
    let cursor = writer.compaction_cursor();
    writer.checkpoint().unwrap();
    assert!(writer
        .stage_compaction_tail(cursor, 20, &root.join("tail"))
        .is_err());
    writer.apply(DeltaChange::Remove("/after".into())).unwrap();
    assert!(writer.is_removed("/after").unwrap());
    drop(writer);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn tail_staging_refuses_to_overwrite_an_existing_file() {
    let root = std::env::temp_dir().join(format!("ns-compaction-refuse-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let mut writer = DeltaIndex::open(&root.join("base.delta"), 10).unwrap();
    let cursor = writer.compaction_cursor();
    writer.apply(DeltaChange::Remove("/after".into())).unwrap();
    let staged = root.join("tail");
    std::fs::write(&staged, b"preserve").unwrap();
    assert_eq!(
        writer
            .stage_compaction_tail(cursor, 20, &staged)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::AlreadyExists
    );
    assert_eq!(std::fs::read(staged).unwrap(), b"preserve");
    assert!(writer.is_removed("/after").unwrap());
    drop(writer);
    std::fs::remove_dir_all(root).unwrap();
}
