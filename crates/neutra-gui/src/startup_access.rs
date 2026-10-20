#[cfg(not(unix))]
use std::path::PathBuf;

#[cfg(not(target_os = "windows"))]
pub(crate) fn request_elevated_restart() -> Result<(), String> {
    Err("elevated restart is only available on Windows".into())
}

#[cfg(not(unix))]
pub(crate) fn validate_elevated_helper(path: &std::path::Path) -> Result<PathBuf, String> {
    std::fs::canonicalize(path).map_err(|error| {
        format!(
            "cannot resolve installed helper {}: {error}",
            path.display()
        )
    })
}

#[cfg(target_os = "windows")]
pub(crate) fn request_elevated_restart() -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;

    #[allow(non_snake_case)]
    #[link(name = "shell32")]
    extern "system" {
        fn ShellExecuteW(
            window: *mut std::ffi::c_void,
            operation: *const u16,
            file: *const u16,
            parameters: *const u16,
            directory: *const u16,
            show: i32,
        ) -> *mut std::ffi::c_void;
    }

    let executable = std::env::current_exe()
        .map_err(|error| format!("cannot locate Neutrasearch executable: {error}"))?;
    let operation = std::ffi::OsStr::new("runas")
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let executable = executable
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            executable.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
        )
    } as isize;
    if result <= 32 {
        Err(format!(
            "Windows elevation request failed with code {result}"
        ))
    } else {
        Ok(())
    }
}
