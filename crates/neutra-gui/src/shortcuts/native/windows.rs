use std::os::windows::process::CommandExt;
use std::{ffi::OsStr, os::windows::ffi::OsStrExt, process::Command, time::Duration};

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const VALUE_NAME: &str = "NeutrasearchShortcutAgent";

pub(super) fn run_agent() -> Result<(), String> {
    let _lock = acquire_lock()?;
    super::agent_loop(wait_for_events)
}

pub(super) fn wait_for_events(timeout: Duration) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MsgWaitForMultipleObjectsEx, PeekMessageW, TranslateMessage, MSG,
        MWMO_INPUTAVAILABLE, PM_REMOVE, QS_ALLINPUT,
    };
    let milliseconds = timeout.as_millis().min(u32::MAX as u128) as u32;
    unsafe {
        MsgWaitForMultipleObjectsEx(
            0,
            std::ptr::null(),
            milliseconds,
            QS_ALLINPUT,
            MWMO_INPUTAVAILABLE,
        );
        let mut message: MSG = std::mem::zeroed();
        while PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

pub(super) fn install_autostart() -> Result<(), String> {
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY_CURRENT_USER, KEY_SET_VALUE,
        REG_OPTION_NON_VOLATILE, REG_SZ,
    };
    use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

    let executable =
        std::env::current_exe().map_err(|e| format!("cannot resolve executable: {e}"))?;
    let command = format!(
        "{} --shortcut-agent",
        windows_quote(
            executable
                .to_str()
                .ok_or("The application path cannot be represented as Unicode")?
        )
    );
    let mut key = std::ptr::null_mut();
    let result = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            wide(RUN_KEY).as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        )
    };
    if result != 0 {
        return Err(format!(
            "cannot open per-user startup registry key: Windows error {result}"
        ));
    }
    let mut data = wide(&command);
    let result = unsafe {
        RegSetValueExW(
            key,
            wide(VALUE_NAME).as_ptr(),
            0,
            REG_SZ,
            data.as_mut_ptr().cast(),
            (data.len() * 2) as u32,
        )
    };
    unsafe {
        RegCloseKey(key);
    }
    if result != 0 {
        return Err(format!(
            "cannot register per-user shortcut startup: Windows error {result}"
        ));
    }

    let status = Command::new(executable)
        .arg("--shortcut-agent")
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("cannot start shortcut agent: {e}"))?;
    drop(status);
    Ok(())
}

fn wide(value: &str) -> Vec<u16> {
    OsStr::new(value).encode_wide().chain(Some(0)).collect()
}

struct AgentLock(windows_sys::Win32::Foundation::HANDLE);

impl Drop for AgentLock {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

fn acquire_lock() -> Result<AgentLock, String> {
    use windows_sys::Win32::{
        Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE},
        Storage::FileSystem::{CreateFileW, FILE_ATTRIBUTE_NORMAL, OPEN_ALWAYS},
    };
    let path = super::super::config_path().with_extension("lock");
    std::fs::create_dir_all(path.parent().ok_or("Shortcut lock has no parent")?)
        .map_err(|e| e.to_string())?;
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            std::ptr::null(),
            OPEN_ALWAYS,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(format!(
            "Cannot acquire the shortcut agent lock: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(AgentLock(handle))
}

fn windows_quote(value: &str) -> String {
    let mut quoted = String::from("\"");
    let mut slashes = 0;
    for character in value.chars() {
        match character {
            '\\' => slashes += 1,
            '"' => {
                quoted.push_str(&"\\".repeat(slashes * 2 + 1));
                quoted.push('"');
                slashes = 0;
            }
            other => {
                quoted.push_str(&"\\".repeat(slashes));
                quoted.push(other);
                slashes = 0;
            }
        }
    }
    quoted.push_str(&"\\".repeat(slashes * 2));
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use super::windows_quote;
    #[test]
    fn quotes_windows_executable_paths_with_spaces_and_trailing_slashes() {
        assert_eq!(
            windows_quote(r"C:\Program Files\Neutra\app.exe"),
            r#""C:\Program Files\Neutra\app.exe""#
        );
        assert_eq!(
            windows_quote(r#"C:\folder\"name"\app.exe"#),
            r#""C:\folder\\\"name\"\app.exe""#
        );
    }
}
