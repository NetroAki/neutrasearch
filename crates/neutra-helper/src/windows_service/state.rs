use anyhow::{Context, Result};
use std::ffi::c_void;
use std::{
    ffi::OsStr,
    os::windows::ffi::OsStrExt,
    path::PathBuf,
    ptr::null_mut,
    sync::atomic::{AtomicBool, AtomicPtr},
};

pub(super) const SERVICE_NAME: &str = "NeutrasearchHelper";
pub(super) const PIPE_PATH: &str = r"\\.\pipe\Neutrasearch.Helper.v1";
pub(super) static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);
pub(super) static STATUS_HANDLE: AtomicPtr<c_void> = AtomicPtr::new(null_mut());
pub(super) static SERVICE_THREAD: AtomicPtr<c_void> = AtomicPtr::new(null_mut());

/// Log inside the installation directory, whose ACL the installer restricts to
/// SYSTEM and administrators. A world-creatable location such as ProgramData
/// would let an ordinary user pre-create or redirect what this account writes.
fn service_log_directory() -> Result<PathBuf> {
    let executable = std::env::current_exe().context("locate the service executable")?;
    let directory = executable
        .parent()
        .context("service executable has no directory")?
        .join("logs");
    std::fs::create_dir_all(&directory).context("create service log directory")?;
    refuse_reparse_point(&directory)?;
    Ok(directory)
}

fn refuse_reparse_point(path: &std::path::Path) -> Result<()> {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 => {
            anyhow::bail!("refusing reparse point {}", path.display())
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("inspect {}", path.display())),
    }
}

pub(super) fn init_service_logging() -> Result<()> {
    let directory = service_log_directory()?;
    let path = directory.join("helper.log");
    refuse_reparse_point(&path)?;
    if std::fs::metadata(&path).is_ok_and(|metadata| metadata.len() > 2 * 1024 * 1024) {
        let _ = std::fs::rename(&path, directory.join("helper.previous.log"));
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open service log {}", path.display()))?;
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(std::sync::Mutex::new(file))
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "neutra_helper=info".into()),
        )
        .init();
    Ok(())
}

pub(super) fn service_log(message: &str) {
    tracing::info!(target: "neutra_helper::windows_service", "{message}");
}

pub(super) fn wide(value: &str) -> Vec<u16> {
    OsStr::new(value).encode_wide().chain(Some(0)).collect()
}
