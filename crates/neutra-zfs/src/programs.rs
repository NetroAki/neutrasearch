use anyhow::{bail, Context, Result};
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(super) fn trusted(path: &Path) -> Result<PathBuf> {
    let path = secure(path)?;
    if path.metadata()?.mode() & 0o111 == 0 {
        bail!("ZFS metadata program is not executable");
    }
    Ok(path)
}

fn secure(path: &Path) -> Result<PathBuf> {
    let path = path
        .canonicalize()
        .with_context(|| format!("Missing ZFS metadata program {}", path.display()))?;
    let uid = unsafe { libc::geteuid() };
    let metadata = path.metadata()?;
    if !metadata.is_file() {
        bail!("ZFS metadata program or library is not a regular file");
    }
    for component in path.ancestors() {
        let meta = component.metadata()?;
        if (meta.uid() != 0 && meta.uid() != uid) || meta.mode() & 0o022 != 0 {
            bail!(
                "ZFS metadata program is in an untrusted writable location: {}",
                component.display()
            );
        }
    }
    Ok(path)
}

pub(super) fn reader() -> Result<PathBuf> {
    let executable = std::env::current_exe()?;
    let parent = executable
        .parent()
        .context("No helper installation directory")?;
    let sibling = parent.join("neutrasearch-zfs");
    let candidate = if !sibling.exists() && parent.file_name().is_some_and(|name| name == "helpers")
    {
        parent
            .parent()
            .context("No packaged helper root")?
            .join("neutrasearch-zfs")
    } else {
        sibling
    };
    let reader =
        trusted(&candidate).context("The installation is missing its trusted native ZFS reader")?;
    let runtime = reader
        .parent()
        .context("No ZFS reader directory")?
        .join("zfs-runtime");
    for library in [
        "libzpool.so.5",
        "libzfs.so.4",
        "libzfs_core.so.3",
        "libnvpair.so.3",
        "libuutil.so.3",
    ] {
        secure(&runtime.join(library))
            .context("The installation is missing its trusted OpenZFS runtime")?;
    }
    Ok(reader)
}

pub(super) fn command(path: &Path) -> Command {
    let mut command = Command::new(path);
    for variable in ["LD_PRELOAD", "LD_LIBRARY_PATH", "LD_AUDIT"] {
        command.env_remove(variable);
    }
    // SAFETY: the hook performs only capget/capset/prctl syscalls and uses
    // stack-local data; it does not allocate, lock, or touch inherited Rust state.
    unsafe {
        command.pre_exec(crate::capability::inherit_zfs_capabilities);
    }
    command
}
