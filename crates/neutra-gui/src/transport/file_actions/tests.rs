#[cfg(unix)]
use super::clipboard::{gio_uri, url_to_path};
use super::trash::{restore_with_env, trash_with_env};
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
static NEXT: AtomicU64 = AtomicU64::new(0);
static TRASH_TEST_LOCK: Mutex<()> = Mutex::new(());
fn sandbox() -> (PathBuf, PathBuf) {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let base = home.join(format!(
        ".ns-fop-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let data = base.join("data");
    std::fs::create_dir_all(&data).unwrap();
    (base, data)
}
#[test]
fn no_replace_rename_preserves_both_files() {
    let (d, _) = sandbox();
    let a = d.join("a");
    let b = d.join("b");
    std::fs::write(&a, "a").unwrap();
    std::fs::write(&b, "b").unwrap();
    assert!(rename_noreplace(&a, &b).is_err());
    assert_eq!(std::fs::read(&a).unwrap(), b"a");
    assert_eq!(std::fs::read(&b).unwrap(), b"b");
    std::fs::remove_dir_all(d).unwrap();
}
#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires a desktop GIO trash service; run separately in the owned GUI test session"]
fn trash_and_restore_round_trip_and_refuse_overwrite() {
    let _lock = TRASH_TEST_LOCK.lock().unwrap();
    let (base, data) = sandbox();
    let root = base.join("source");
    std::fs::create_dir_all(&root).unwrap();
    let f = root.join("sample file.txt");
    std::fs::write(&f, "source").unwrap();
    let uri = trash_with_env(&f, Some(&data)).unwrap();
    assert!(!f.exists(), "trash must remove the original");
    std::fs::write(&f, "preserve").unwrap();
    assert!(restore_with_env(&uri, Some(&data)).is_err());
    assert_eq!(std::fs::read(&f).unwrap(), b"preserve");
    std::fs::remove_file(&f).unwrap();
    restore_with_env(&uri, Some(&data)).unwrap();
    assert_eq!(std::fs::read(&f).unwrap(), b"source");
    let _ = std::fs::remove_dir_all(&base);
}
#[cfg(unix)]
#[test]
fn file_uri_encodes_spaces_and_unicode() {
    let p = Path::new("/tmp/a space/é.txt");
    let uri = gio_uri(p).unwrap();
    assert_eq!(url_to_path(&uri).unwrap(), p);
}
