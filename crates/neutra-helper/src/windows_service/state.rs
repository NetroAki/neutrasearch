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

pub(super) fn init_service_logging() -> Result<()> {
    let directory = std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
        .join("Neutrasearch");
    std::fs::create_dir_all(&directory).context("create service log directory")?;
    let path = directory.join("helper.log");
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
