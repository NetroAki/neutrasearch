use super::*;

pub(crate) fn compaction_stage(base: &std::path::Path) -> std::path::PathBuf {
    append_suffix(base, ".compact")
}

pub(crate) fn compaction_marker(base: &std::path::Path) -> std::path::PathBuf {
    append_suffix(base, ".compacting")
}

pub(crate) fn compaction_marker_temp(base: &std::path::Path) -> std::path::PathBuf {
    append_suffix(base, ".compacting.new")
}

pub(crate) fn stale_marker(base: &std::path::Path) -> std::path::PathBuf {
    append_suffix(base, ".stale")
}

#[cfg(target_os = "linux")]
pub(crate) fn write_watch_status(base: &std::path::Path, text: &str) -> Result<()> {
    let path = append_suffix(base, ".watch-status");
    let temp = append_suffix(&path, ".new");
    let _ = std::fs::remove_file(&temp);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(&temp)?;
    file.write_all(text.as_bytes())?;
    drop(file);
    std::fs::rename(temp, path)?;
    Ok(())
}

pub(crate) fn write_stale_marker(base: &std::path::Path, reason: &str) -> Result<()> {
    let marker = stale_marker(base);
    if marker.is_file() {
        return Ok(());
    }
    let temporary = append_suffix(&marker, ".new");
    let _ = std::fs::remove_file(&temporary);
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let mut file = options.open(&temporary)?;
    let reason = reason.as_bytes();
    file.write_all(&reason[..reason.len().min(4096)])?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temporary, &marker)?;
    sync_parent(&marker)
}

pub(crate) fn append_suffix(path: &std::path::Path, suffix: &str) -> std::path::PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    value.into()
}

pub(crate) fn write_compaction_marker(path: &std::path::Path, generation: u64) -> Result<()> {
    if path.exists() {
        anyhow::bail!("compaction marker already exists: {}", path.display());
    }
    let temporary = append_suffix(path, ".new");
    let _ = std::fs::remove_file(&temporary);
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .with_context(|| format!("create compaction marker {}", temporary.display()))?;
    file.write_all(&generation.to_le_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temporary, path).with_context(|| {
        format!(
            "publish compaction marker {} -> {}",
            temporary.display(),
            path.display()
        )
    })?;
    sync_parent(path)
}

pub(crate) fn read_compaction_marker(path: &std::path::Path) -> Result<u64> {
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("open compaction marker {}", path.display()))?;
    if file.metadata()?.len() != 8 {
        anyhow::bail!("invalid compaction marker length");
    }
    let mut generation = [0u8; 8];
    file.read_exact(&mut generation)?;
    let generation = u64::from_le_bytes(generation);
    if generation == 0 {
        anyhow::bail!("invalid zero compaction generation");
    }
    Ok(generation)
}

pub(crate) fn remove_compaction_marker(path: &std::path::Path) -> Result<()> {
    std::fs::remove_file(path)
        .with_context(|| format!("remove compaction marker {}", path.display()))?;
    let _ = std::fs::remove_file(append_suffix(path, ".new"));
    sync_parent(path)
}

#[cfg(unix)]
pub(crate) fn sync_parent(path: &std::path::Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn sync_parent(_path: &std::path::Path) -> Result<()> {
    Ok(())
}
