use super::*;
use crate::{FileKind, FsKind};

fn record(path: &str, source: u32) -> FileRecord {
    FileRecord {
        path: path.into(),
        size: 0,
        mtime: 0,
        mode: 0,
        kind: FileKind::File,
        fs: FsKind::Ext4,
        native_id: 0,
        native_parent: 0,
        source,
        disk: 0,
    }
}

const PATHS: [&str; 8] = [
    "/t/alpha",
    "/t/Beta",
    "/t/beta",
    "/t/a\\b",
    "/t/a/b",
    "/t/a/B",
    "/t/dir/File",
    "/t/dir/file",
];

/// The heap that merges sorted runs must order heads exactly as the run
/// sort did; any difference interleaves runs out of index order.
#[test]
fn merge_keys_order_like_the_run_sort() {
    for order in [SpillOrder::Compact, SpillOrder::Summary] {
        for (i, left) in PATHS.iter().enumerate() {
            for (j, right) in PATHS.iter().enumerate() {
                let (l, r) = (record(left, 1), record(right, 1));
                let expected = compare(order, &l, &r).then(i.cmp(&j));
                let actual = key_for(order, &l, i).cmp(&key_for(order, &r, j));
                assert_eq!(actual, expected, "{left} vs {right}");
            }
        }
    }
}

#[test]
fn summary_keys_order_by_source_before_path() {
    let low = key_for(SpillOrder::Summary, &record("/z", 1), 0);
    let high = key_for(SpillOrder::Summary, &record("/a", 2), 1);
    assert!(low < high);
}
