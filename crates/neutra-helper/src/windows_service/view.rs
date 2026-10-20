use super::{
    pipe,
    state::{service_log, wide, SERVICE_NAME, SERVICE_THREAD, STATUS_HANDLE, STOP_REQUESTED},
};
use anyhow::{Context, Result};
use std::{
    ffi::c_void,
    ptr::{null, null_mut},
    sync::atomic::Ordering,
};
use windows_sys::Win32::Foundation::{GetLastError, HANDLE};
use windows_sys::Win32::System::Services::{
    RegisterServiceCtrlHandlerExW, SetServiceStatus, StartServiceCtrlDispatcherW,
    SERVICE_ACCEPT_STOP, SERVICE_CONTROL_STOP, SERVICE_RUNNING, SERVICE_START_PENDING,
    SERVICE_STATUS, SERVICE_STATUS_HANDLE, SERVICE_STOPPED, SERVICE_STOP_PENDING,
    SERVICE_TABLE_ENTRYW, SERVICE_WIN32_OWN_PROCESS,
};
use windows_sys::Win32::System::Threading::{GetCurrentThreadId, OpenThread, THREAD_TERMINATE};
use windows_sys::Win32::System::IO::CancelSynchronousIo;

pub(crate) fn run() -> Result<()> {
    let mut name = wide(SERVICE_NAME);
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: name.as_mut_ptr(),
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW::default(),
    ];
    let ok = unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) };
    if ok == 0 {
        return Err(std::io::Error::last_os_error()).context("start Windows service dispatcher");
    }
    Ok(())
}

unsafe extern "system" fn service_main(_argc: u32, _argv: *mut windows_sys::core::PWSTR) {
    let name = wide(SERVICE_NAME);
    let handle =
        unsafe { RegisterServiceCtrlHandlerExW(name.as_ptr(), Some(control_handler), null()) };
    if handle.is_null() {
        return;
    }
    STATUS_HANDLE.store(handle.cast(), Ordering::Release);
    let thread = unsafe { OpenThread(THREAD_TERMINATE, 0, GetCurrentThreadId()) };
    if thread.is_null() {
        report_status(SERVICE_STOPPED, unsafe { GetLastError() }, 0);
        STATUS_HANDLE.store(null_mut(), Ordering::Release);
        return;
    }
    // CancelSynchronousIo requires a real thread handle, not the pseudo-handle
    // returned by GetCurrentThread. The service process exits after this body,
    // so Windows closes this handle during process teardown.
    SERVICE_THREAD.store(thread.cast(), Ordering::Release);
    report_status(SERVICE_START_PENDING, 0, 3_000);

    let result = pipe::service_body();
    if let Err(error) = &result {
        service_log(&format!("service stopped after error: {error:#}"));
    }
    let exit_code = i32::from(result.is_err());
    report_status(SERVICE_STOPPED, exit_code as u32, 0);
    STATUS_HANDLE.store(null_mut(), Ordering::Release);
    // This executable hosts exactly one SCM service. A stop cancels the
    // service thread's blocking pipe I/O; exiting then cancels any read-only
    // native scan worker still parsing
    // the volume. The GUI keeps those records in staging and never publishes a
    // scan whose completion frame was interrupted.
    std::process::exit(exit_code);
}

unsafe extern "system" fn control_handler(
    control: u32,
    _event_type: u32,
    _event_data: *mut c_void,
    _context: *mut c_void,
) -> u32 {
    if control == SERVICE_CONTROL_STOP {
        STOP_REQUESTED.store(true, Ordering::Release);
        report_status(SERVICE_STOP_PENDING, 0, 5_000);
        // ConnectNamedPipe and protocol reads are synchronous. Cancel the
        // service thread's current I/O so it can observe STOP_REQUESTED and
        // report SERVICE_STOPPED without waiting for a client connection.
        let thread = SERVICE_THREAD.load(Ordering::Acquire) as HANDLE;
        if !thread.is_null() {
            unsafe {
                CancelSynchronousIo(thread);
            }
        }
    }
    0
}

pub(super) fn report_status(state: u32, exit_code: u32, wait_hint: u32) {
    let handle = STATUS_HANDLE.load(Ordering::Acquire) as SERVICE_STATUS_HANDLE;
    if handle.is_null() {
        return;
    }
    let status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: if state == SERVICE_RUNNING {
            SERVICE_ACCEPT_STOP
        } else {
            0
        },
        dwWin32ExitCode: exit_code,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: 0,
        dwWaitHint: wait_hint,
    };
    unsafe {
        SetServiceStatus(handle, &status);
    }
}
