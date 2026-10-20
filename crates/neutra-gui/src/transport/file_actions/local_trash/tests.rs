use super::*;
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};

struct Sandbox(PathBuf);
impl Sandbox {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "ns-local-trash-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(fs::canonicalize(root).unwrap())
    }
    fn source(&self) -> PathBuf {
        let path = self.0.join("é % music.txt");
        fs::write(&path, b"original contents").unwrap();
        path
    }
}
impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn unicode_roundtrip_preserves_contents_and_refuses_conflict() {
    let sandbox = Sandbox::new();
    let source = sandbox.source();
    let token = move_to_trash(&source, &sandbox.0).unwrap();
    assert!(!source.exists());
    assert!(fs::read_to_string(&token)
        .unwrap()
        .contains("Path=%C3%A9%20%25%20music.txt"));
    fs::write(&source, b"keep destination").unwrap();
    assert!(restore(&token).is_err());
    assert!(token.exists());
    assert_eq!(fs::read(&source).unwrap(), b"keep destination");
    fs::remove_file(&source).unwrap();
    assert_eq!(restore(&token).unwrap(), source);
    assert_eq!(fs::read(&source).unwrap(), b"original contents");
    assert!(!token.exists());
}

#[test]
fn symlink_is_moved_without_following_its_target() {
    let sandbox = Sandbox::new();
    let target = sandbox.source();
    let link = sandbox.0.join("link");
    symlink(&target, &link).unwrap();
    let token = move_to_trash(&link, &sandbox.0).unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"original contents");
    assert!(fs::symlink_metadata(&link).is_err());
    restore(&token).unwrap();
    assert_eq!(fs::read_link(&link).unwrap(), target);
}

#[test]
fn unsafe_trash_directories_leave_source_untouched() {
    let sandbox = Sandbox::new();
    let source = sandbox.source();
    let trash = storage::prepare(&sandbox.0).unwrap();
    fs::set_permissions(&trash, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(move_to_trash(&source, &sandbox.0).is_err());
    fs::set_permissions(&trash, fs::Permissions::from_mode(0o700)).unwrap();
    fs::remove_dir(trash.join("files")).unwrap();
    symlink(&sandbox.0, trash.join("files")).unwrap();
    assert!(move_to_trash(&source, &sandbox.0).is_err());
    assert_eq!(fs::read(&source).unwrap(), b"original contents");
}

#[test]
fn restore_rejects_escaping_or_ambiguous_metadata() {
    let sandbox = Sandbox::new();
    let source = sandbox.source();
    let token = move_to_trash(&source, &sandbox.0).unwrap();
    for path in [
        "../outside",
        "%2E%2E/outside",
        "/outside",
        "%00",
        "%GG",
        "good\nPath=other",
    ] {
        fs::write(&token, format!("[Trash Info]\nPath={path}\n")).unwrap();
        assert!(restore(&token).is_err(), "accepted {path}");
        assert!(token.exists());
        assert!(!source.exists());
    }
}

#[test]
fn restore_rejects_symlinked_shared_or_oversized_metadata() {
    let sandbox = Sandbox::new();
    let source = sandbox.source();
    let token = move_to_trash(&source, &sandbox.0).unwrap();
    let alias = sandbox.0.join("metadata");
    fs::hard_link(&token, &alias).unwrap();
    assert!(restore(&token).is_err());
    fs::remove_file(&alias).unwrap();
    fs::write(&token, "x".repeat(65_537)).unwrap();
    assert!(restore(&token).is_err());
    fs::rename(&token, &alias).unwrap();
    symlink(&alias, &token).unwrap();
    assert!(restore(&token).is_err());
    assert!(!source.exists());
}

#[test]
fn directory_roundtrip_does_not_touch_nested_files() {
    let sandbox = Sandbox::new();
    let folder = sandbox.0.join("folder");
    fs::create_dir(&folder).unwrap();
    fs::write(folder.join("child"), b"nested contents").unwrap();
    let inode = fs::metadata(&folder).unwrap().ino();
    let token = move_to_trash(&folder, &sandbox.0).unwrap();
    restore(&token).unwrap();
    assert_eq!(fs::metadata(&folder).unwrap().ino(), inode);
    assert_eq!(fs::read(folder.join("child")).unwrap(), b"nested contents");
}
