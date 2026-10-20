use super::domain::*;
use super::view::scan_image;
use neutra_core::FileKind;

#[test]
fn inode_layout() {
    assert_eq!(std::mem::size_of::<Ext2Inode>(), 128);
    assert_eq!(std::mem::size_of::<DirEntry>(), 264);
}
#[test]
fn modes() {
    assert_eq!(kind(0o100644), FileKind::File);
    assert_eq!(kind(0o040755), FileKind::Dir);
}
#[test]
fn scans_real_ext4_image_without_mounting() {
    if std::process::Command::new("mke2fs")
        .arg("-V")
        .output()
        .is_err()
    {
        return;
    }
    let base = std::env::temp_dir().join(format!("neutra-ext4-test-{}", std::process::id()));
    let root = base.join("root");
    let image = base.join("fixture.ext4");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(root.join("nested")).unwrap();
    std::fs::write(root.join("hello.txt"), b"hello").unwrap();
    std::fs::write(root.join("nested/code.rs"), b"fn main() {}\n").unwrap();
    let f = std::fs::File::create(&image).unwrap();
    f.set_len(16 * 1024 * 1024).unwrap();
    drop(f);
    let status = std::process::Command::new("mke2fs")
        .args(["-q", "-t", "ext4", "-d"])
        .arg(&root)
        .arg(&image)
        .status()
        .unwrap();
    assert!(status.success());
    let mut records = Vec::new();
    let stats = scan_image(&image, "/fixture", &mut |r| records.push(r)).unwrap();
    assert!(stats.records >= 3);
    assert!(records
        .iter()
        .any(|r| r.path.as_ref() == "/fixture/hello.txt" && r.size == 5));
    assert!(records
        .iter()
        .any(|r| r.path.as_ref() == "/fixture/nested/code.rs" && r.kind == FileKind::File));
    let _ = std::fs::remove_dir_all(base);
}
