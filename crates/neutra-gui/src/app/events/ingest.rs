use super::NeutraApp;
use neutra_core::proto::HelperMsg;

pub(super) enum ScanIngest {
    Records(usize),
    Failed(String),
    LaneEvent,
    Complete,
}

pub(super) fn ingest_scan_message(
    staging: &mut Option<neutra_core::SpillAccumulator>,
    failed: &mut bool,
    roots: &[std::path::PathBuf],
    message: HelperMsg,
) -> ScanIngest {
    match message {
        HelperMsg::Records(records) => {
            let batch = records
                .into_iter()
                .filter(|record| crate::record_in_roots(record.path.as_ref(), roots))
                .collect::<Vec<_>>();
            let count = batch.len();
            if let Some(spill) = staging {
                if let Err(error) = spill.push_batch(batch) {
                    *failed = true;
                    *staging = None;
                    return ScanIngest::Failed(error.to_string());
                }
            }
            ScanIngest::Records(count)
        }
        HelperMsg::ScanError { .. } => {
            *failed = true;
            *staging = None;
            ScanIngest::LaneEvent
        }
        HelperMsg::ScanDone { .. } => ScanIngest::LaneEvent,
        HelperMsg::ScanComplete { mounts, errors } => {
            if mounts == 0 || errors > 0 || *failed {
                *staging = None;
                *failed = true;
            }
            ScanIngest::Complete
        }
        _ => ScanIngest::LaneEvent,
    }
}

/// Attribute a filtered batch to mounts by longest mountpoint prefix, for
/// live per-drive progress. Runs in the event pump, amortized per batch.
pub(super) fn count_staged(app: &mut NeutraApp, batch: &[neutra_core::FileRecord]) {
    if batch.is_empty() {
        return;
    }
    let mounts: Vec<String> = app
        .lanes
        .keys()
        .filter(|key| key.starts_with('/'))
        .cloned()
        .collect();
    for record in batch {
        if let Some(mount) = longest_mount(&mounts, record.path.as_ref()) {
            *app.staged_by_mount.entry(mount).or_default() += 1;
        }
    }
}

pub(super) fn longest_mount(mounts: &[String], path: &str) -> Option<String> {
    mounts
        .iter()
        .filter(|mount| {
            mount.as_str() == "/"
                || path == mount.as_str()
                || path.starts_with(&format!("{mount}/"))
        })
        .max_by_key(|mount| mount.len())
        .cloned()
}
