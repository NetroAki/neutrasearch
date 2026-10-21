use super::*;

#[test]
fn concurrent_ingest_cannot_remove_an_active_spill() {
    let output = std::env::temp_dir().join(format!("neutra-spill-lock-{}.nsx", std::process::id()));
    let first = SpillAccumulator::begin(&output).unwrap();
    let sentinel = output.with_extension("spill").join("active.run");
    std::fs::write(&sentinel, b"active build").unwrap();
    assert!(
        matches!(SpillAccumulator::begin(&output), Err(error) if error.kind() == io::ErrorKind::WouldBlock)
    );
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"active build");
    let runs = first.finish().unwrap();
    assert!(
        matches!(SpillAccumulator::begin(&output), Err(error) if error.kind() == io::ErrorKind::WouldBlock)
    );
    drop(runs);
    drop(SpillAccumulator::begin(&output).unwrap());
    std::fs::remove_file(output.with_extension("spill.lock")).unwrap();
}

#[test]
fn unrelated_spill_directory_and_output_directory_are_preserved() {
    let output =
        std::env::temp_dir().join(format!("neutra-unowned-spill-{}.nsx", std::process::id()));
    let directory = output.with_extension("spill");
    std::fs::create_dir(&directory).unwrap();
    let sentinel = directory.join("user-data");
    std::fs::write(&sentinel, b"keep").unwrap();
    assert!(SpillAccumulator::begin(&output).is_err());
    assert!(SpillAccumulator::begin(&directory).is_err());
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"keep");
    std::fs::remove_file(sentinel).unwrap();
    std::fs::remove_dir(directory).unwrap();
    std::fs::remove_file(output.with_extension("spill.lock")).unwrap();
}

#[test]
fn owned_interrupted_spill_is_recovered() {
    let output =
        std::env::temp_dir().join(format!("neutra-owned-spill-{}.nsx", std::process::id()));
    let mut spill = SpillAccumulator::begin(&output).unwrap();
    let directory = spill.dir.take().unwrap();
    std::fs::write(directory.join("interrupted.run"), b"old records").unwrap();
    drop(spill);
    let recovered = SpillAccumulator::begin(&output).unwrap();
    assert!(!directory.join("interrupted.run").exists());
    drop(recovered);
    std::fs::remove_file(output.with_extension("spill.lock")).unwrap();
}

#[cfg(unix)]
#[test]
fn ingest_and_sorted_run_files_are_private_from_creation() {
    use std::os::unix::fs::PermissionsExt;
    let output =
        std::env::temp_dir().join(format!("neutra-private-spill-{}.nsx", std::process::id()));
    let mut spill = SpillAccumulator::begin(&output).unwrap();
    spill
        .push_batch(vec![FileRecord {
            path: "/private/file".into(),
            size: 1,
            disk: 1,
            mtime: 0,
            mode: 0,
            kind: crate::FileKind::File,
            fs: crate::FsKind::Ext4,
            native_id: 1,
            native_parent: 0,
            source: 0,
        }])
        .unwrap();
    let runs = spill.finish().unwrap();
    assert_eq!(
        std::fs::metadata(runs.dir().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    let merge =
        crate::compact_merge::RunMerge::open(&runs, crate::compact_merge::SpillOrder::Compact)
            .unwrap();
    let paths = [
        runs.chunks()[0].clone(),
        runs.dir().unwrap().join("run-c-000000.srt"),
    ];
    for path in paths {
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    drop(merge);
    drop(runs);
    std::fs::remove_file(output.with_extension("spill.lock")).unwrap();
}
