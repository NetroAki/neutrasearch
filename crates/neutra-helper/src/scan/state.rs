#[cfg(target_os = "linux")]
use crate::store::{
    append_suffix, compaction_marker, compaction_marker_temp, compaction_stage, stale_marker,
};

pub(crate) fn reap_scan_threads(threads: &mut Vec<std::thread::JoinHandle<()>>) {
    let mut index = 0;
    while index < threads.len() {
        if threads[index].is_finished() {
            let thread = threads.swap_remove(index);
            if thread.join().is_err() {
                tracing::error!("native scan worker panicked");
            }
        } else {
            index += 1;
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn watch_exclusions(base: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut delta = base.to_path_buf();
    delta.set_extension("delta");
    let mut lock = delta.as_os_str().to_os_string();
    lock.push(".lock");
    let temporary = append_suffix(base, ".new");
    let staged = compaction_stage(base);
    let staged_temporary = append_suffix(&staged, ".new");
    let marker = compaction_marker(base);
    let marker_temporary = compaction_marker_temp(base);
    let stale = stale_marker(base);
    let stale_temporary = append_suffix(&stale, ".new");
    let overlay_paths = neutra_core::DeltaIndex::exclusion_roots(&delta);
    let mut paths = vec![
        base.to_path_buf(),
        append_suffix(&delta, ".checkpoint"),
        append_suffix(&delta, ".migrate"),
        base.with_extension("spill"),
        base.with_extension("spill.lock"),
        delta,
        lock.into(),
        temporary,
        staged,
        staged_temporary,
        marker,
        marker_temporary,
        append_suffix(base, ".compact-tail"),
        append_suffix(base, ".compact-tail.new"),
        stale,
        stale_temporary,
    ];
    paths.extend(overlay_paths);
    for path in [base.to_path_buf(), compaction_stage(base)] {
        paths.push(path.with_extension("spill"));
        paths.push(path.with_extension("spill.lock"));
        for suffix in [
            ".browse",
            ".browse.new",
            ".browse.lock",
            ".browse.base",
            ".browse.cols",
            ".browse.cols.new",
            ".browse.cols.new.runs",
            ".browse.base.new",
            ".browse-wal",
            ".browse-shm",
            ".dirs",
            ".dirs.new",
            ".tree",
            ".tree.new",
            ".rank",
            ".rank.new",
            ".sweep",
            ".sweep.new",
            ".sweep.tmp",
            ".watch-status",
            ".watch-status.new",
        ] {
            paths.push(append_suffix(&path, suffix));
        }
    }
    paths
}

#[cfg(target_os = "linux")]
pub(crate) fn is_watch_excluded(path: &std::path::Path, roots: &[std::path::PathBuf]) -> bool {
    roots.iter().any(|root| {
        if path.starts_with(root) {
            return true;
        }
        if path.parent() != root.parent() {
            return false;
        }
        let (Some(name), Some(stem)) = (
            path.file_name().and_then(|s| s.to_str()),
            root.file_name().and_then(|s| s.to_str()),
        ) else {
            return false;
        };
        let Some(suffix) = name.strip_prefix(stem) else {
            return false;
        };
        if stem.ends_with(".new") {
            if let Some((pid, nonce)) = suffix.strip_prefix('-').and_then(|s| s.split_once('-')) {
                return pid.parse::<u32>().is_ok() && nonce.parse::<u128>().is_ok();
            }
        }
        suffix
            .strip_prefix(".publish-")
            .and_then(|s| s.strip_suffix(".new"))
            .is_some_and(|generation| generation.parse::<u64>().is_ok())
    })
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    #[test]
    fn compaction_artifacts_do_not_become_source_events() {
        let base = std::path::Path::new("/state/home.nsx");
        let roots = watch_exclusions(base);
        for suffix in [
            ".spill/chunk-000001.run",
            ".spill.lock",
            ".compact.new",
            ".compact.dirs.new-123-456",
            ".publish-123.new",
        ] {
            assert!(
                is_watch_excluded(&append_suffix(base, suffix), &roots),
                "{suffix}"
            );
        }
        for path in [
            "/state/photo.jpg",
            "/state/home.nsx.backup",
            "/state/home.nsx.publish-user.new",
            "/state/home.nsx.compact.dirs.new-user-file",
        ] {
            assert!(
                !is_watch_excluded(std::path::Path::new(path), &roots),
                "{path}"
            );
        }
    }
}
