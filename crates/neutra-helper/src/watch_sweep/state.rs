use super::*;
use anyhow::Context;

pub(super) fn open_subvolumes(mounts: &[MountInfo]) -> Result<Vec<Watched>> {
    let mut out = Vec::new();
    for mount in mounts {
        if mount.fs != FsKind::Btrfs {
            continue;
        }
        let sub = open_for(mount).ok_or_else(|| {
            anyhow!(
                "cannot open native Btrfs metadata at {}",
                mount.mountpoint.display()
            )
        })?;
        let since = sub.current_generation()?;
        let prefix = mount
            .mountpoint
            .to_string_lossy()
            .trim_end_matches('/')
            .to_string();
        let relative_root = sub.opened_dir_path()?;
        tracing::info!(mount = %mount.mountpoint.display(), tree = sub.tree_id(), "sweeping Btrfs subvolume");
        out.push(Watched {
            sub,
            mount: mount.clone(),
            prefix,
            relative_root,
            since,
            active: false,
        });
    }
    Ok(out)
}

/// Merge the base-generation seed with any durable sweep cursor. Seed entries
/// must identify exactly one currently mounted Btrfs subvolume; otherwise we
/// cannot prove that native catch-up covers the published base.
pub(super) fn apply_seed(
    watched: &mut [Watched],
    cursors: &[neutra_core::NativeCursor],
) -> Result<()> {
    anyhow::ensure!(
        watched.len() == cursors.len(),
        "ingest cursor does not cover every mounted Btrfs subvolume"
    );
    let mut matched = vec![false; watched.len()];
    for cursor in cursors {
        let index = watched
            .iter()
            .position(|item| {
                item.sub.tree_id() == cursor.tree
                    && item.mount.mountpoint.to_string_lossy() == cursor.mount
            })
            .ok_or_else(|| anyhow!("ingest cursor subvolume is not mounted: {}", cursor.mount))?;
        anyhow::ensure!(
            !matched[index],
            "duplicate ingest cursor for {}",
            cursor.mount
        );
        matched[index] = true;
        let item = &mut watched[index];
        item.since = item.since.min(cursor.since);
        item.active = true;
    }
    for (item, found) in watched.iter_mut().zip(matched.iter()) {
        if !found {
            item.active = true;
        }
    }
    anyhow::ensure!(
        matched.into_iter().all(|value| value),
        "ingest cursor does not cover every mounted Btrfs subvolume"
    );
    Ok(())
}

/// Open a mount's subvolume, going through the user's home when the mount
/// root itself is not readable by the user and both sit on the same mount.
fn open_for(mount: &MountInfo) -> Option<Subvolume> {
    if let Ok(sub) = Subvolume::open(&mount.mountpoint) {
        return Some(sub);
    }
    use std::os::unix::fs::MetadataExt;
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let same =
        std::fs::metadata(&home).ok()?.dev() == std::fs::metadata(&mount.mountpoint).ok()?.dev();
    same.then(|| Subvolume::open(&home).ok()).flatten()
}

/// Keep each mount's last safely reconciled cursor across compaction/rebuild.
pub(super) fn restore(watched: &mut [Watched], state: &Path, seed_id: Option<u32>) -> Result<bool> {
    let text = match std::fs::read_to_string(state) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let mut lines = text.lines();
    let first = lines.next().unwrap_or_default();
    let lines = if let Some(id) = first.strip_prefix("NTRSWP2 ") {
        let saved = id
            .parse::<u32>()
            .context("invalid native recovery seed identity")?;
        if Some(saved) != seed_id {
            return Ok(false);
        }
        lines
    } else {
        if seed_id.is_some() {
            return Ok(false);
        }
        text.lines()
    };
    let mut matched = vec![false; watched.len()];
    for line in lines {
        let mut parts = line.splitn(3, ' ');
        let (Some(tree), Some(since)) = (
            parts.next().and_then(|v| v.parse::<u64>().ok()),
            parts.next().and_then(|v| v.parse::<u64>().ok()),
        ) else {
            anyhow::bail!("invalid native recovery cursor");
        };
        anyhow::ensure!(
            tree != 0 && since != 0,
            "invalid zero native recovery cursor"
        );
        if let Some(mount) = parts.next() {
            if let Some((index, item)) = watched.iter_mut().enumerate().find(|(_, item)| {
                item.sub.tree_id() == tree && item.mount.mountpoint.to_string_lossy() == mount
            }) {
                item.since = if seed_id.is_some() {
                    since.min(item.sub.current_generation()?)
                } else {
                    item.since.min(since)
                };
                matched[index] = true;
            }
        } else {
            for (index, item) in watched
                .iter_mut()
                .enumerate()
                .filter(|(_, item)| item.sub.tree_id() == tree)
            {
                item.since = if seed_id.is_some() {
                    since.min(item.sub.current_generation()?)
                } else {
                    item.since.min(since)
                };
                matched[index] = true;
            }
        }
    }
    Ok(matched.into_iter().all(|found| found))
}

#[cfg(test)]
mod tests;

pub(super) fn save(watched: &[Watched], state: &Path, seed_id: u32) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut text = String::new();
    text.push_str(&format!("NTRSWP2 {seed_id}\n"));
    for item in watched {
        text.push_str(&format!(
            "{} {} {}\n",
            item.sub.tree_id(),
            item.since,
            item.mount.mountpoint.to_string_lossy()
        ));
    }
    let temporary = append_suffix(state, ".tmp");
    match std::fs::remove_file(&temporary) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&temporary)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    std::fs::rename(&temporary, state)?;
    std::fs::File::open(state.parent().context("recovery cursor has no parent")?)?.sync_all()?;
    Ok(())
}
