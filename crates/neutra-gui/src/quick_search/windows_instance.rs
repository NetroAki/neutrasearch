use std::{
    io,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE},
    System::Threading::CreateMutexW,
    UI::WindowsAndMessaging::{FindWindowW, SetForegroundWindow, ShowWindow, SW_RESTORE},
};

pub(super) struct Guard(HANDLE);

impl Drop for Guard {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

pub(super) fn claim_or_focus() -> io::Result<Option<Guard>> {
    let name: Vec<u16> = "Local\\NeutrasearchQuickSearch"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    let exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    let guard = Guard(handle);
    if !exists {
        return Ok(Some(guard));
    }
    let title: Vec<u16> = "Neutrasearch Quick Search"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let window = unsafe { FindWindowW(std::ptr::null(), title.as_ptr()) };
        if !window.is_null() {
            unsafe {
                ShowWindow(window, SW_RESTORE);
                SetForegroundWindow(window);
            }
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "The existing quick-search window is still opening",
    ))
}
