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
