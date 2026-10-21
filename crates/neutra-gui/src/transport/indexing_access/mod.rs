use std::path::{Path, PathBuf};
use std::process::Command;

mod process;

const GRANTS: &str = "/var/lib/neutrasearch/authorized-users";

pub(crate) fn installed_helper() -> Option<PathBuf> {
    ["/usr/local/lib/neutrasearch", "/usr/lib/neutrasearch"]
        .into_iter()
        .map(|directory| Path::new(directory).join("neutrasearch-helper"))
        .find_map(|path| super::validate_elevated_helper(&path).ok())
}

fn isolated() -> bool {
    [
        "NEUTRASEARCH_HELPER",
        "NEUTRASEARCH_INDEX",
        "NEUTRASEARCH_MANAGED_WATCH",
    ]
    .iter()
    .any(|name| std::env::var_os(name).is_some())
}

pub(crate) fn retained() -> bool {
    use std::os::unix::fs::MetadataExt;
    let uid = unsafe { libc::getuid() };
    std::fs::symlink_metadata(Path::new(GRANTS).join(uid.to_string()))
        .is_ok_and(|meta| meta.is_file() && meta.uid() == 0 && meta.mode() & 0o022 == 0)
}

fn executable_has_access(helper: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(path) = std::ffi::CString::new(helper.as_os_str().as_bytes()) else {
        return false;
    };
    let mut caps = [0u8; 24];
    let length = unsafe {
        libc::getxattr(
            path.as_ptr(),
            c"security.capability".as_ptr(),
            caps.as_mut_ptr().cast(),
            caps.len(),
        )
    };
    if length < 20 {
        return false;
    }
    let flags = u32::from_le_bytes(caps[0..4].try_into().unwrap());
    let permitted = u32::from_le_bytes(caps[4..8].try_into().unwrap());
    let required = (1 << 2) | (1 << 21);
    flags & 1 != 0
        && permitted & required == required
        && unsafe { libc::access(path.as_ptr(), libc::X_OK) } == 0
}

pub(crate) fn authorize(helper: &Path) -> Result<(), String> {
    enable(helper, false)
}

fn enable(helper: &Path, repair: bool) -> Result<(), String> {
    if !repair && retained() && executable_has_access(helper) {
        return Ok(());
    }
    let authorizer = helper.with_file_name("neutrasearch-authorize");
    let authorizer = super::validate_elevated_helper(&authorizer).map_err(|_| {
        "Indexing access is unavailable. Reinstall Neutrasearch to restore its access manager."
            .to_owned()
    })?;
    let mut command = Command::new("/usr/bin/pkexec");
    command.arg(authorizer);
    process::run(command, 120).map_err(|error| format!(
        "Indexing access was not enabled: {error}. Your previous index is safe. Choose Enable indexing in Index settings to try again."
    ))?;
    if !retained() || !executable_has_access(helper) {
        return Err("The system did not retain indexing access. Choose Repair indexing access in Index settings.".into());
    }
    Ok(())
}

pub(crate) fn control_watcher(operation: &str) -> Result<(), String> {
    if isolated() {
        return Ok(());
    }
    if !matches!(operation, "start" | "stop" | "restart") {
        return Err("Unsupported indexing service operation".into());
    }
    let mut command = Command::new("/usr/bin/systemctl");
    command.args([
        "--no-ask-password",
        operation,
        &format!("neutrasearch-watch@{}.service", unsafe { libc::getuid() }),
    ]);
    process::run(command, 30)
}

pub(crate) fn resume(tx: crate::app::EventSender) {
    if isolated() || !retained() {
        return;
    }
    std::thread::spawn(move || {
        let result = control_watcher("start");
        report(tx, result, "Live indexing enabled");
    });
}

pub(crate) fn repair(tx: crate::app::EventSender) {
    if isolated() {
        return;
    }
    std::thread::spawn(move || {
        let result = installed_helper()
            .ok_or_else(|| "The installed scanner is missing. Reinstall Neutrasearch.".into())
            .and_then(|helper| enable(&helper, true))
            .and_then(|()| control_watcher("restart"));
        report(
            tx,
            result,
            "Indexing access retained; live indexing enabled",
        );
    });
}

fn report(tx: crate::app::EventSender, result: Result<(), String>, success: &str) {
    let error = result.is_err();
    let status = result.err().unwrap_or_else(|| success.to_owned());
    let _ = tx.send(crate::Event::Remote {
        key: "indexing".into(),
        status,
        error,
    });
}

pub(crate) fn resume_after_exit() {
    static SCHEDULED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if isolated() || !retained() || SCHEDULED.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return;
    }
    let Some(helper) = installed_helper() else {
        return;
    };
    let watcher = helper.with_file_name("neutrasearch-watch");
    if let Err(error) = Command::new(watcher)
        .args(["--resume-after", &std::process::id().to_string()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        eprintln!("Cannot schedule live indexing after closing: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unrecognized_service_operations() {
        if !isolated() {
            assert!(control_watcher("enable arbitrary.service").is_err());
        }
    }
}
