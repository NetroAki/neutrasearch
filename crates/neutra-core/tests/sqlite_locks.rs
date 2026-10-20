#![cfg(target_os = "linux")]
use neutra_core::{DeltaChange, DeltaIndex};
use std::{os::fd::AsRawFd, path::Path, process::Command};

#[test]
fn snapshot_open_preserves_database_locks() {
    if let Some(path) = std::env::var_os("NEUTRA_SQLITE_LOCK_PATH") {
        probe(Path::new(&path));
        return;
    }
    let root = std::env::temp_dir().join(format!("ns-sqlite-locks-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let path = root.join("base.delta");
    let cache = root.join(".delta.overlay/base.delta.resolved.sqlite");
    let mut writer = DeltaIndex::open(&path, 10).unwrap();
    writer
        .apply(DeltaChange::Remove("/missing".into()))
        .unwrap();
    writer.sync().unwrap();
    let first = DeltaIndex::open_search_snapshot(&path, 10).unwrap();
    assert_locked(&cache);
    let second = DeltaIndex::open_search_snapshot(&path, 10).unwrap();
    assert_locked(&cache);
    let private = DeltaIndex::open_snapshot(&path, 10).unwrap();
    assert_locked(&cache);
    drop(private);
    drop(second);
    drop(first);
    drop(writer);
    std::fs::remove_dir_all(root).unwrap();
}

fn assert_locked(path: &Path) {
    let status = Command::new("/usr/bin/timeout")
        .arg("5")
        .arg(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "snapshot_open_preserves_database_locks",
            "--nocapture",
        ])
        .env("NEUTRA_SQLITE_LOCK_PATH", path)
        .status()
        .unwrap();
    assert!(
        status.success(),
        "snapshot opener released a live SQLite lock"
    );
}

fn probe(path: &Path) {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    let mut lock = libc::flock {
        l_type: libc::F_WRLCK as _,
        l_whence: libc::SEEK_SET as _,
        l_start: 0x40000002,
        l_len: 510,
        l_pid: 0,
    };
    // SQLite's shared database lock must still exclude an external writer.
    let result = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETLK, &mut lock) };
    assert_eq!(result, -1, "live database has lost its POSIX read lock");
    let error = std::io::Error::last_os_error();
    assert!(matches!(
        error.raw_os_error(),
        Some(libc::EAGAIN) | Some(libc::EACCES)
    ));
}
