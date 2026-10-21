use std::{fs::File, io, os::windows::io::AsRawHandle, ptr::null};
use windows_sys::Win32::{
    Foundation::HANDLE,
    System::{
        Pipes::GetNamedPipeServerProcessId,
        Services::{
            CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceStatusEx, SC_HANDLE,
            SC_MANAGER_CONNECT, SC_STATUS_PROCESS_INFO, SERVICE_QUERY_STATUS, SERVICE_RUNNING,
            SERVICE_STATUS_PROCESS,
        },
    },
};

struct ServiceHandle(SC_HANDLE);

impl Drop for ServiceHandle {
    fn drop(&mut self) {
        unsafe { CloseServiceHandle(self.0) };
    }
}

pub(super) fn validate_server(pipe: &File) -> io::Result<()> {
    let mut server_pid = 0;
    if unsafe { GetNamedPipeServerProcessId(pipe.as_raw_handle() as HANDLE, &mut server_pid) } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let manager = unsafe { OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT) };
    if manager.is_null() {
        return Err(io::Error::last_os_error());
    }
    let manager = ServiceHandle(manager);
    let name: Vec<u16> = "NeutrasearchHelper\0".encode_utf16().collect();
    let service = unsafe { OpenServiceW(manager.0, name.as_ptr(), SERVICE_QUERY_STATUS) };
    if service.is_null() {
        return Err(io::Error::last_os_error());
    }
    let service = ServiceHandle(service);
    let mut status = SERVICE_STATUS_PROCESS::default();
    let mut bytes = 0;
    if unsafe {
        QueryServiceStatusEx(
            service.0,
            SC_STATUS_PROCESS_INFO,
            (&mut status as *mut SERVICE_STATUS_PROCESS).cast(),
            std::mem::size_of_val(&status) as u32,
            &mut bytes,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if server_pid == 0
        || server_pid != status.dwProcessId
        || status.dwCurrentState != SERVICE_RUNNING
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "scanner pipe does not belong to the running service",
        ));
    }
    Ok(())
}
