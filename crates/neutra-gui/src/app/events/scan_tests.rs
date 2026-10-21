use super::{ingest_scan_message, ScanIngest};
use neutra_core::proto::HelperMsg;
use neutra_core::{
    CompactIndex, FileKind, FileRecord, FsKind, MountInfo, MountSource, ScanStats, SpillAccumulator,
};
use std::path::{Path, PathBuf};

fn mount(path: &str) -> MountInfo {
    MountInfo {
        device: "/dev/test".into(),
        mountpoint: PathBuf::from(path),
        fs: FsKind::Ext4,
        source: MountSource::Local,
    }
}

fn record(path: &str) -> FileRecord {
    FileRecord {
        path: path.into(),
        size: 17,
        disk: 17,
        mtime: 0,
        mode: 0,
        kind: FileKind::File,
        fs: FsKind::Ext4,
        native_id: 1,
        native_parent: 0,
        source: 0,
    }
}

fn temp_path(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "neutrasearch-scan-event-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn publish_one(path: &Path, record: FileRecord) -> u64 {
    let mut spill = SpillAccumulator::begin(path).unwrap();
    spill.push_batch(vec![record]).unwrap();
    CompactIndex::rebuild_streamed(spill.finish().unwrap(), path).unwrap();
    CompactIndex::open_fast(path).unwrap().generation()
}

#[test]
fn errored_records_are_dropped_before_later_lane_success_and_completion() {
    let prior_path = temp_path("prior");
    let staging_path = temp_path("staging");
    let prior_generation = publish_one(&prior_path, record("/mnt/old/complete.txt"));
    let prior_len = CompactIndex::open_fast(&prior_path).unwrap().len();

    let roots = vec![PathBuf::from("/mnt/failed"), PathBuf::from("/mnt/good")];
    let mut staging = Some(SpillAccumulator::begin(&staging_path).unwrap());
    let mut failed = false;

    assert!(matches!(
        ingest_scan_message(
            &mut staging,
            &mut failed,
            &roots,
            HelperMsg::Records(vec![record("/mnt/failed/partial.txt")]),
        ),
        ScanIngest::Records(1)
    ));
    assert!(matches!(
        ingest_scan_message(
            &mut staging,
            &mut failed,
            &roots,
            HelperMsg::ScanError {
                mount: mount("/mnt/failed"),
                error: "native reader failed".into(),
            },
        ),
        ScanIngest::LaneEvent
    ));
    assert!(matches!(
        ingest_scan_message(
            &mut staging,
            &mut failed,
            &roots,
            HelperMsg::ScanDone {
                mount: mount("/mnt/good"),
                stats: ScanStats::default(),
            },
        ),
        ScanIngest::LaneEvent
    ));
    assert!(matches!(
        ingest_scan_message(
            &mut staging,
            &mut failed,
            &roots,
            HelperMsg::ScanComplete {
                mounts: 2,
                errors: 1,
            },
        ),
        ScanIngest::Complete
    ));

    assert!(failed);
    assert!(staging.is_none());
    let prior = CompactIndex::open_fast(&prior_path).unwrap();
    assert_eq!(prior.generation(), prior_generation);
    assert_eq!(prior.len(), prior_len);
    assert!(!staging_path.with_extension("spill").exists());

    drop(prior);
    let _ = std::fs::remove_file(prior_path);
}
