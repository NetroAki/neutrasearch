use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT: AtomicU64 = AtomicU64::new(0);

#[test]
fn rename_preserves_an_existing_destination() {
    let root = sandbox();
    let source = root.join("source");
    let target = root.join("target");
    std::fs::write(&source, "source bytes").unwrap();
    std::fs::write(&target, "keep these bytes").unwrap();
    assert!(rename_noreplace(&source, &target).is_err());
    assert_eq!(std::fs::read(&source).unwrap(), b"source bytes");
    assert_eq!(std::fs::read(&target).unwrap(), b"keep these bytes");
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn folder_copy_collision_preserves_source_and_destination() {
    let root = sandbox();
    let source = root.join("source");
    let target = root.join("target");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("child"), "source bytes").unwrap();
    std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("child"), "keep these bytes").unwrap();
    assert!(copy_path_noreplace(&source, &target).is_err());
    assert_eq!(
        std::fs::read(source.join("child")).unwrap(),
        b"source bytes"
    );
    assert_eq!(
        std::fs::read(target.join("child")).unwrap(),
        b"keep these bytes"
    );
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 2);
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn missing_folder_source_leaves_no_target_or_staging() {
    let root = sandbox();
    let source = root.join("missing");
    let target = root.join("target");
    assert!(copy_path_noreplace(&source, &target).is_err());
    assert!(!target.exists());
    assert!(std::fs::read_dir(&root).unwrap().next().is_none());
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(not(target_os = "linux"))]
#[test]
fn folder_copy_rejects_its_own_descendant_before_creating_it() {
    let root = sandbox();
    let folder = root.join("folder");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("keep"), "source bytes").unwrap();
    let target = folder.join("nested");
    assert!(copy_path_noreplace(&folder, &target).is_err());
    assert!(!target.exists());
    assert_eq!(std::fs::read(folder.join("keep")).unwrap(), b"source bytes");
    std::fs::remove_dir_all(root).unwrap();
}

fn sandbox() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "ns-transfer-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&root).unwrap();
    root
}
#[test]
fn refuses_existing_destination_without_touching_files() {
    let root = sandbox();
    let source = root.join("source");
    let target = root.join("target");
    std::fs::write(&source, "source bytes").unwrap();
    std::fs::write(&target, "keep these bytes").unwrap();
    assert!(copy_path_noreplace(&source, &target).is_err());
    assert_eq!(std::fs::read(&source).unwrap(), b"source bytes");
    assert_eq!(std::fs::read(&target).unwrap(), b"keep these bytes");
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn transfer_copies_files_folders_and_batch_without_overwrite() {
    let root = sandbox();
    let src = root.join("src");
    let dst = root.join("dst");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&dst).unwrap();
    let file = src.join("note.txt");
    std::fs::write(&file, "original").unwrap();
    let folder = src.join("bundle");
    std::fs::create_dir_all(folder.join("nested")).unwrap();
    std::fs::write(folder.join("nested/file.txt"), "deep").unwrap();
    std::fs::write(dst.join("note.txt"), "keep").unwrap();
    let batch = vec![file.clone(), folder.clone()];
    assert!(transfer(&batch, &dst, false).is_err());
    assert_eq!(std::fs::read(&file).unwrap(), b"original");
    assert_eq!(std::fs::read(dst.join("note.txt")).unwrap(), b"keep");
    assert!(!dst.join("bundle").exists());
    std::fs::remove_file(dst.join("note.txt")).unwrap();
    transfer(&batch, &dst, false).unwrap();
    assert_eq!(std::fs::read(dst.join("note.txt")).unwrap(), b"original");
    assert_eq!(
        std::fs::read(dst.join("bundle/nested/file.txt")).unwrap(),
        b"deep"
    );
    assert!(file.exists() && folder.exists());
    assert!(transfer(std::slice::from_ref(&file), &dst, true).is_err());
    assert!(file.exists());
    let moved = src.join("solo.txt");
    std::fs::write(&moved, "solo").unwrap();
    transfer(std::slice::from_ref(&moved), &dst, true).unwrap();
    assert!(!moved.exists());
    assert_eq!(std::fs::read(dst.join("solo.txt")).unwrap(), b"solo");
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn failed_staged_copy_leaves_no_partial_destination() {
    use std::os::unix::fs::PermissionsExt;
    let root = sandbox();
    let source = root.join("source");
    let target = root.join("target");
    std::fs::create_dir_all(source.join("locked")).unwrap();
    std::fs::write(source.join("first"), "bytes").unwrap();
    std::fs::set_permissions(
        source.join("locked"),
        std::fs::Permissions::from_mode(0o000),
    )
    .unwrap();
    let unreadable = std::fs::read_dir(source.join("locked")).is_err();
    let outcome = copy::copy(&source, &target);
    std::fs::set_permissions(
        source.join("locked"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    if unreadable {
        assert!(outcome.is_err());
        assert!(!target.exists());
        let leftovers: Vec<_> = std::fs::read_dir(&root).unwrap().collect();
        assert_eq!(leftovers.len(), 1, "staging was not cleaned: {leftovers:?}");
    } else {
        assert!(outcome.is_ok());
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn staged_copy_publishes_the_whole_tree_without_replacing() {
    let root = sandbox();
    let source = root.join("source");
    std::fs::create_dir_all(source.join("inner")).unwrap();
    std::fs::write(source.join("inner/file"), "bytes").unwrap();
    let target = root.join("target");
    copy::copy(&source, &target).unwrap();
    assert_eq!(std::fs::read(target.join("inner/file")).unwrap(), b"bytes");
    assert!(copy::copy(&source, &target).is_err());
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 2);
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(windows)]
#[test]
fn windows_copy_keeps_file_streams_and_refuses_folders_with_streams() {
    let root = sandbox();
    let source = root.join("source");
    std::fs::write(&source, "main").unwrap();
    std::fs::write(format!("{}:note", source.display()), "stream").unwrap();
    let target = root.join("target");
    copy_path_noreplace(&source, &target).unwrap();
    assert_eq!(
        std::fs::read(format!("{}:note", target.display())).unwrap(),
        b"stream"
    );
    let folder = root.join("folder");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(format!("{}:note", folder.display()), "stream").unwrap();
    assert!(copy_path_noreplace(&folder, &root.join("folder-copy")).is_err());
    assert!(!root.join("folder-copy").exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn macos_copy_keeps_extended_attributes() {
    let root = sandbox();
    let source = root.join("source");
    std::fs::write(&source, "main").unwrap();
    assert!(Command::new("xattr")
        .args(["-w", "com.neutrasearch.test", "kept"])
        .arg(&source)
        .status()
        .unwrap()
        .success());
    let target = root.join("target");
    copy_path_noreplace(&source, &target).unwrap();
    let out = Command::new("xattr")
        .args(["-p", "com.neutrasearch.test"])
        .arg(&target)
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "kept");
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn cross_volume_move_copies_removes_the_source_and_keeps_late_changes() {
    use std::os::unix::fs::MetadataExt;
    let shm = PathBuf::from("/dev/shm");
    let disk = sandbox();
    if !shm.is_dir()
        || std::fs::metadata(&shm).unwrap().dev() == std::fs::metadata(&disk).unwrap().dev()
    {
        std::fs::remove_dir_all(disk).unwrap();
        return;
    }
    let source = shm.join(format!("ns-move-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&source);
    std::fs::create_dir_all(source.join("inner")).unwrap();
    std::fs::write(source.join("inner/data"), "payload").unwrap();
    let target = disk.join("moved");
    move_path_noreplace(&source, &target).unwrap();
    assert_eq!(
        std::fs::read(target.join("inner/data")).unwrap(),
        b"payload"
    );
    assert!(!source.exists());
    std::fs::remove_dir_all(disk).unwrap();
}
