use super::{
    init_service_logging, report_status, service_log, validate_installed_service_files,
    verify_client, wide, PIPE_PATH, STOP_REQUESTED,
};
use anyhow::{Context, Result};
use std::{fs::File, os::windows::io::FromRawHandle, ptr::null_mut, sync::atomic::Ordering};
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, LocalFree, ERROR_PIPE_CONNECTED, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows_sys::Win32::System::Services::SERVICE_RUNNING;

pub(super) fn service_body() -> Result<()> {
    init_service_logging()?;
    validate_installed_service_files()?;
    service_log("service starting");
    report_status(SERVICE_RUNNING, 0, 0);

    while !STOP_REQUESTED.load(Ordering::Acquire) {
        let pipe = create_pipe().context("create privileged scanner pipe")?;
        service_log("pipe created; waiting for client");
        let connected = unsafe { ConnectNamedPipe(pipe, null_mut()) };
        if connected == 0 && unsafe { GetLastError() } != ERROR_PIPE_CONNECTED {
            unsafe { CloseHandle(pipe) };
            if STOP_REQUESTED.load(Ordering::Acquire) {
                break;
            }
            return Err(std::io::Error::last_os_error()).context("accept scanner pipe client");
        }
        if STOP_REQUESTED.load(Ordering::Acquire) {
            unsafe {
                DisconnectNamedPipe(pipe);
                CloseHandle(pipe);
            }
            break;
        }

        service_log("pipe client connected; entering protocol");
        // File owns the pipe handle from here. The protocol sends Hello before
        // authenticating the client because process-image inspection can block.
        // No command is read until this callback has accepted the client.
        let mut reader = unsafe { File::from_raw_handle(pipe.cast()) };
        let writer = reader.try_clone().context("clone scanner pipe handle")?;
        service_log("pipe handles cloned; starting protocol Hello exchange");
        let authenticate = || verify_client(pipe);
        if let Err(error) = crate::protocol::run_protocol_with_auth(
            &mut reader,
            Box::new(writer),
            None,
            None,
            Some(&STOP_REQUESTED),
            Some(&authenticate),
        ) {
            if !STOP_REQUESTED.load(Ordering::Acquire) {
                service_log(&format!("rejected pipe client: {error:#}"));
            }
        }
        unsafe {
            DisconnectNamedPipe(pipe);
        }
    }

    service_log("service stopped");
    Ok(())
}

fn create_pipe() -> Result<HANDLE> {
    // SYSTEM and Administrators have full control. Authenticated local users
    // may connect, but remote clients are rejected and the executable path is
    // authenticated immediately after connection.
    let sddl = wide("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;AU)");
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    let converted = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            null_mut(),
        )
    };
    if converted == 0 {
        return Err(std::io::Error::last_os_error()).context("build scanner pipe ACL");
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let path = wide(PIPE_PATH);
    let handle = unsafe {
        CreateNamedPipeW(
            path.as_ptr(),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            64 * 1024,
            64 * 1024,
            5_000,
            &attributes,
        )
    };
    unsafe {
        LocalFree(descriptor);
    }
    if handle == INVALID_HANDLE_VALUE {
        Err(std::io::Error::last_os_error()).context("create scanner named pipe")
    } else {
        Ok(handle)
    }
}
