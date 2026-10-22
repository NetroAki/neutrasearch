use super::*;
use neutra_core::{BrowserIndex, DeltaChange, DeltaIndex, FsKind};
use std::path::Path;

pub(super) fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(2).collect();
    let [index, target] = args.as_slice() else {
        anyhow::bail!("internal usage: --repair-mount INDEX MOUNT");
    };
    let index = std::fs::canonicalize(index)?;
    let target = std::fs::canonicalize(target)?;
    let layout = std::fs::read_to_string("/proc/self/mountinfo")?;
    let mount = neutra_core::mounts::parse_mountinfo(&layout)
        .into_iter()
        .find(|mount| mount.mountpoint == target)
        .context("target is not a supported mounted filesystem")?;
    anyhow::ensure!(
        mount.fs == FsKind::Ext4,
        "targeted recovery is qualified for ext4 only"
    );
    validate_layout(&layout, &mount.mountpoint)?;
    refresh(&index, &mount.mountpoint, |sink| {
        let stats = dispatch_lane(&mount, sink)?;
        anyhow::ensure!(
            layout == std::fs::read_to_string("/proc/self/mountinfo")?,
            "mount layout changed during native recovery"
        );
        println!(
            "native metadata recovered: {} records in {} ms",
            stats.records, stats.wall_ms
        );
        Ok(())
    })
}

fn validate_layout(raw: &str, mount: &Path) -> Result<()> {
    anyhow::ensure!(
        neutra_core::mounts::mount_root(raw, mount).as_deref() == Some("/"),
        "targeted recovery requires a whole filesystem mount, not a subtree bind mount"
    );
    anyhow::ensure!(
        !neutra_core::mounts::mountpoints(raw)
            .iter()
            .any(|point| point != mount && point.starts_with(mount)),
        "targeted recovery cannot replace a mount containing nested mounts"
    );
    Ok(())
}

fn refresh(
    index: &Path,
    mount: &Path,
    scan: impl FnOnce(&mut dyn FnMut(neutra_core::FileRecord)) -> Result<()>,
) -> Result<()> {
    let marker = crate::store::stale_marker(index);
    anyhow::ensure!(
        marker.is_file(),
        "offline recovery requires a retained stale marker"
    );
    let candidate = CompactIndex::open_for_recovery(index)?;
    let mut delta = DeltaIndex::open(&index.with_extension("delta"), candidate.generation())?;
    drop(candidate);
    anyhow::ensure!(
        marker.is_file(),
        "recovery marker changed before writer ownership"
    );
    for suffix in [".compacting", ".compact-tail", ".compact"] {
        anyhow::ensure!(
            !crate::store::append_suffix(index, suffix).exists(),
            "unfinished compaction must be recovered first"
        );
    }
    let base = CompactIndex::open_for_recovery(index)?;
    anyhow::ensure!(
        base.generation() == delta.generation(),
        "base changed before writer ownership"
    );
    let browser = BrowserIndex::open(index, base.generation())?;
    if !browser.covers_delta(index)? {
        BrowserIndex::apply_delta(index, &delta)?;
    }
    let root = mount.to_str().context("mount path is not UTF-8")?;
    let exclusions = crate::scan::watch_exclusions(index);
    let mut batch = vec![DeltaChange::RemovePrefix(root.into())];
    let mut failed = None;
    let scanned = scan(&mut |record| {
        if failed.is_some() {
            return;
        }
        let path = Path::new(&*record.path);
        if !path.starts_with(mount) {
            failed = Some(anyhow::anyhow!(
                "native reader emitted a path outside its mount"
            ));
            return;
        }
        if crate::scan::is_watch_excluded(path, &exclusions) {
            return;
        }
        batch.push(DeltaChange::Upsert(record));
        if batch.len() >= 1024 {
            if let Err(error) = commit(index, &mut delta, std::mem::take(&mut batch)) {
                failed = Some(error);
            }
        }
    });
    scanned?;
    if let Some(error) = failed {
        return Err(error);
    }
    if !batch.is_empty() {
        commit(index, &mut delta, batch)?;
    }
    BrowserIndex::mark_delta(index, &delta)?;
    anyhow::ensure!(
        marker.is_file(),
        "recovery marker was changed by another process"
    );
    Ok(())
}

fn commit(index: &Path, delta: &mut DeltaIndex, batch: Vec<DeltaChange>) -> Result<()> {
    delta.apply_batch(batch.iter().cloned())?;
    delta.sync()?;
    BrowserIndex::apply_changes(index, delta.generation(), &batch)?;
    Ok(())
}

#[cfg(test)]
mod tests;
