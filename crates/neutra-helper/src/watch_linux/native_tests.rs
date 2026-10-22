use super::*;
use std::io::Write;
use std::time::Duration;

fn drain(watcher: &mut FanotifyWatcher) -> Vec<DeltaChange> {
    let mut changes = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while watcher.wait_readable(Duration::from_millis(30)).unwrap() {
        assert!(
            std::time::Instant::now() < deadline,
            "event stream did not drain"
        );
        match watcher.read_batch().unwrap() {
            WatchBatch::Changes(batch) => changes.extend(batch),
            WatchBatch::ReconcileRequired(reason) | WatchBatch::RescanRequired(reason) => {
                panic!("native coverage lost: {reason}");
            }
        }
    }
    changes
}

#[test]
#[ignore = "requires CAP_SYS_ADMIN and an isolated ext4 mount in NEUTRA_TEST_EXT4_MOUNT"]
fn ext4_deletions_across_reads_keep_native_coverage() {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    let root =
        PathBuf::from(std::env::var_os("NEUTRA_TEST_EXT4_MOUNT").expect("isolated ext4 mount"));
    let fixture = root.join(format!("neutra-watch-{}", std::process::id()));
    std::fs::create_dir(&fixture).unwrap();
    let mount = MountInfo {
        device: "native-test".into(),
        mountpoint: fixture.clone(),
        fs: neutra_core::FsKind::Ext4,
        source: neutra_core::MountSource::Local,
    };
    let mut watcher = FanotifyWatcher::open(mount, 0, Vec::new()).unwrap();
    for n in 0..100 {
        let parent = fixture.join(format!("parent-{n}"));
        std::fs::create_dir(&parent).unwrap();
        let path = parent.join("transient");
        let mut file = File::create(&path).unwrap();
        file.write_all(b"test").unwrap();
        drain(&mut watcher);
        std::fs::remove_file(&path).unwrap();
        let removed = drain(&mut watcher);
        assert!(removed.iter().any(|change| matches!(change, DeltaChange::Remove(gone) if gone.as_ref() == path.to_str().unwrap())));
        std::fs::remove_dir(&parent).unwrap();
        drop(file);
        let removed = drain(&mut watcher);
        assert!(removed.iter().any(|change| matches!(change, DeltaChange::RemovePrefix(gone) if gone.as_ref() == parent.to_str().unwrap())));
    }
    for n in 0..100 {
        let parent = fixture.join(format!("nested-{n}"));
        let nested = parent.join("child");
        std::fs::create_dir_all(&nested).unwrap();
        let path = nested.join("transient");
        let mut file = File::create(&path).unwrap();
        file.write_all(b"nested deletion").unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&nested).unwrap();
        std::fs::remove_dir(&parent).unwrap();
        drop(file);
        let removed = drain(&mut watcher);
        assert!(removed.iter().any(|change| matches!(change, DeltaChange::RemovePrefix(gone) if gone.as_ref() == parent.to_str().unwrap())));
    }
    std::fs::remove_dir(&fixture).unwrap();
}
