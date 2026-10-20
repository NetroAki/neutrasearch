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
pub(super) fn restore(watched: &mut [Watched], state: &Path) -> Result<()> {
    let text = match std::fs::read_to_string(state) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for line in text.lines() {
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
            if let Some(item) = watched.iter_mut().find(|item| {
                item.sub.tree_id() == tree && item.mount.mountpoint.to_string_lossy() == mount
            }) {
                item.since = item.since.min(since);
            }
        } else {
            for item in watched.iter_mut().filter(|item| item.sub.tree_id() == tree) {
                item.since = item.since.min(since);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;

pub(super) fn save(watched: &[Watched], state: &Path) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut text = String::new();
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
