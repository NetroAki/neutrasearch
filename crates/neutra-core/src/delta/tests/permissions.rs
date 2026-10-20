use super::*;

#[cfg(unix)]
#[test]
fn persistent_overlay_rejects_symlink_and_hardlink_database_files() {
    use std::os::unix::fs::symlink;
    let path = std::env::temp_dir().join(format!(
        "neutra-delta-cache-safe-{}.wal",
        std::process::id()
    ));
    remove_log(&path);
    let root = path.parent().unwrap().join(".delta.overlay");
    std::fs::create_dir_all(&root).unwrap();
    let db = root.join(format!(
        "{}.resolved.sqlite",
        path.file_name().unwrap().to_string_lossy()
    ));
    let victim = root.join(format!("victim-{}", std::process::id()));
    let _ = std::fs::remove_file(&db);
    let _ = std::fs::remove_file(&victim);
    std::fs::write(&victim, b"preserve").unwrap();
    symlink(&victim, &db).unwrap();
    assert!(DeltaIndex::open(&path, 17).is_err());
    assert_eq!(std::fs::read(&victim).unwrap(), b"preserve");
    std::fs::remove_file(&db).unwrap();
    std::fs::hard_link(&victim, &db).unwrap();
    assert!(DeltaIndex::open(&path, 17).is_err());
    assert_eq!(std::fs::read(&victim).unwrap(), b"preserve");
    std::fs::remove_file(&db).unwrap();
    std::fs::remove_file(&victim).unwrap();
    remove_log(&path);
}

#[cfg(unix)]
#[test]
fn writer_refuses_symlinked_wal_and_lock_files() {
    use std::os::unix::fs::symlink;

    let path =
        std::env::temp_dir().join(format!("neutra-delta-symlink-{}.wal", std::process::id()));
    let victim = path.with_extension("victim");
    remove_log(&path);
    let _ = std::fs::remove_file(&victim);
    std::fs::write(&victim, b"do not truncate").unwrap();
    symlink(&victim, &path).unwrap();
    assert!(DeltaIndex::replace_empty(&path, 7).is_err());
    assert_eq!(std::fs::read(&victim).unwrap(), b"do not truncate");
    std::fs::remove_file(&path).unwrap();
    remove_log(&path);

    let lock = lock_path(&path);
    symlink(&victim, &lock).unwrap();
    assert!(DeltaIndex::open(&path, 7).is_err());
    assert_eq!(std::fs::read(&victim).unwrap(), b"do not truncate");
    std::fs::remove_file(lock).unwrap();
    let _ = std::fs::remove_file(path);
    std::fs::remove_file(victim).unwrap();
}
