use super::*;

#[test]
fn keeps_a_same_size_edit_that_restored_its_modification_time() {
    let base = std::env::temp_dir().join(format!("ns-cross-ctime-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    let root = base.join("source");
    fs::create_dir_all(&root).unwrap();
    let file = root.join("f");
    let external = base.join("outside-link");
    fs::write(&file, "aaaa").unwrap();
    fs::hard_link(&file, &external).unwrap();
    let modified = fs::metadata(&file).unwrap().modified().unwrap();
    let snapshot = Snapshot::take(&root).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(&external, "bbbb").unwrap();
    fs::File::options()
        .write(true)
        .open(&external)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    assert_eq!(snapshot.remove_unchanged(&root, &base.join("copy")).left, 2);
    assert_eq!(fs::read(&file).unwrap(), b"bbbb");
    assert_eq!(fs::read(&external).unwrap(), b"bbbb");
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn keeps_files_that_appear_or_change_after_the_snapshot() {
    let root = std::env::temp_dir().join(format!("ns-cross-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("a/b")).unwrap();
    fs::write(root.join("a/same"), "x").unwrap();
    fs::write(root.join("a/b/edited"), "one").unwrap();
    let snapshot = Snapshot::take(&root.join("a")).unwrap();
    fs::write(root.join("a/new"), "late").unwrap();
    fs::write(root.join("a/b/edited"), "two, longer").unwrap();
    assert_eq!(
        snapshot
            .remove_unchanged(&root.join("a"), &root.join("copy"))
            .left,
        3
    );
    assert!(!root.join("a/same").exists());
    assert_eq!(fs::read(root.join("a/new")).unwrap(), b"late");
    assert_eq!(fs::read(root.join("a/b/edited")).unwrap(), b"two, longer");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn removes_a_tree_with_internal_hard_links() {
    let root = std::env::temp_dir().join(format!("ns-cross-links-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("tree")).unwrap();
    fs::write(root.join("tree/one"), "shared").unwrap();
    fs::hard_link(root.join("tree/one"), root.join("tree/two")).unwrap();
    let snapshot = Snapshot::take(&root.join("tree")).unwrap();
    fs::create_dir(root.join("copy")).unwrap();
    fs::write(root.join("copy/one"), "shared").unwrap();
    fs::hard_link(root.join("copy/one"), root.join("copy/two")).unwrap();
    let outcome = snapshot.remove_unchanged(&root.join("tree"), &root.join("copy"));
    assert_eq!(outcome.left, 0);
    assert!(!root.join("tree").exists());
    assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn keeps_a_file_replaced_by_a_new_inode_after_the_snapshot() {
    let root = std::env::temp_dir().join(format!("ns-cross-swap-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("f"), "old").unwrap();
    let snapshot = Snapshot::take(&root).unwrap();
    fs::write(root.join("tmp"), "new!").unwrap();
    fs::rename(root.join("tmp"), root.join("f")).unwrap();
    let outcome = snapshot.remove_unchanged(&root, &root.join("copy"));
    assert_eq!(outcome.left, 2);
    assert_eq!(fs::read(root.join("f")).unwrap(), b"new!");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn keeps_restored_hardlinks_when_the_destination_copied_intermediate_bytes() {
    let base = std::env::temp_dir().join(format!("ns-cross-restored-{}", std::process::id()));
    fs::create_dir_all(base.join("source")).unwrap();
    fs::create_dir_all(base.join("copy")).unwrap();
    let file = base.join("source/one");
    fs::write(&file, "aaaa").unwrap();
    fs::hard_link(&file, base.join("source/two")).unwrap();
    let modified = fs::metadata(&file).unwrap().modified().unwrap();
    let snapshot = Snapshot::take(&base.join("source")).unwrap();
    fs::write(&file, "bbbb").unwrap();
    fs::copy(&file, base.join("copy/one")).unwrap();
    fs::hard_link(base.join("copy/one"), base.join("copy/two")).unwrap();
    fs::write(&file, "aaaa").unwrap();
    fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    let outcome = snapshot.remove_unchanged(&base.join("source"), &base.join("copy"));
    assert_eq!(outcome.left, 3);
    assert_eq!(fs::read(&file).unwrap(), b"aaaa");
    assert_eq!(fs::read(base.join("source/two")).unwrap(), b"aaaa");
    assert_eq!(fs::read(base.join("copy/one")).unwrap(), b"bbbb");
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn digest_rejects_a_fifo_replacing_a_regular_file_without_waiting_for_a_writer() {
    use std::os::unix::ffi::OsStrExt;
    let base = std::env::temp_dir().join(format!("ns-cross-fifo-{}", std::process::id()));
    fs::create_dir_all(&base).unwrap();
    let path = base.join("file");
    fs::write(&path, "regular").unwrap();
    let before = fs::symlink_metadata(&path).unwrap();
    fs::remove_file(&path).unwrap();
    let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    assert!(hardlink_digest::read(&path, &before).is_err());
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn retains_source_edits_made_while_validating_the_destination() {
    use std::io::{Seek, SeekFrom, Write};
    let base = std::env::temp_dir().join(format!("ns-cross-validation-{}", std::process::id()));
    fs::create_dir_all(base.join("source")).unwrap();
    fs::create_dir_all(base.join("copy")).unwrap();
    let source = base.join("source/one");
    fs::write(&source, "aaaa").unwrap();
    fs::hard_link(&source, base.join("source/two")).unwrap();
    let mut writer = fs::File::options().write(true).open(&source).unwrap();
    let snapshot = Snapshot::take(&base.join("source")).unwrap();
    fs::copy(&source, base.join("copy/one")).unwrap();
    fs::hard_link(base.join("copy/one"), base.join("copy/two")).unwrap();
    let outcome =
        snapshot.remove_checked(&base.join("source"), &base.join("copy"), |path, digest| {
            let matches = hardlink_digest::matches(path, digest);
            writer.seek(SeekFrom::Start(0)).unwrap();
            writer.write_all(b"bbbb").unwrap();
            matches
        });
    assert_eq!(outcome.left, 3);
    assert_eq!(fs::read(&source).unwrap(), b"bbbb");
    assert_eq!(fs::read(base.join("source/two")).unwrap(), b"bbbb");
    fs::remove_dir_all(base).unwrap();
}
