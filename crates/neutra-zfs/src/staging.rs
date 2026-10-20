//! Anonymous disk staging for native ZFS streams.
use anyhow::{Context, Result};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

const RAMFS_MAGIC: libc::c_long = 0x8584_58f6u32 as libc::c_long;

pub(super) fn anonymous_spool() -> Result<File> {
    let directory = Path::new("/var/tmp");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_TMPFILE | libc::O_CLOEXEC)
        .mode(0o600)
        .open(directory)
        .with_context(|| format!("Create anonymous ZFS scan spool in {}", directory.display()))?;
    // SAFETY: fstatfs initializes the live output structure for this open file.
    let mut filesystem: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstatfs(file.as_raw_fd(), &mut filesystem) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    anyhow::ensure!(
        filesystem.f_type != libc::TMPFS_MAGIC && filesystem.f_type != RAMFS_MAGIC,
        "ZFS scan staging requires disk-backed /var/tmp; RAM filesystems are unsupported"
    );
    Ok(file)
}

pub(super) fn capture<R: Read, W: Write + Seek>(mut input: R, spool: &mut W) -> Result<()> {
    std::io::copy(&mut input, spool).context("Stage native ZFS metadata stream")?;
    spool.flush().context("Flush staged ZFS metadata stream")?;
    spool.seek(SeekFrom::Start(0))?;
    Ok(())
}

pub(super) fn validate_and_emit<R: Read + Seek>(
    spool: &mut R,
    mountpoint: &Path,
    child_success: bool,
    sink: &mut dyn FnMut(neutra_core::FileRecord),
) -> Result<neutra_core::ScanStats> {
    if !child_success {
        anyhow::bail!("Native ZFS metadata reader exited unsuccessfully");
    }
    super::wire::read(spool, mountpoint, &mut |_| {})?;
    spool.seek(SeekFrom::Start(0))?;
    super::wire::read(spool, mountpoint, sink)
}

#[cfg(test)]
#[path = "staging/tests.rs"]
mod tests;
