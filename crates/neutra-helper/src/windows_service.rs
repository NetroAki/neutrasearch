//! Windows SCM host for the privileged NTFS metadata scanner.
//!
//! The installer places this binary under Program Files and registers it as
//! LocalSystem. A local-only named pipe is the sole IPC surface. Each client
//! must be the installed `neutrasearch.exe` beside this helper; arbitrary
//! processes cannot submit raw protocol messages to the privileged scanner.

use anyhow::{Context, Result};
use std::ffi::{c_void, OsStr};
use std::fs::File;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::FromRawHandle;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, LocalFree, ERROR_PIPE_CONNECTED, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
    GetNamedSecurityInfoW, SDDL_REVISION_1, SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    AclSizeInformation, GetAce, GetAclInformation, ACCESS_ALLOWED_ACE, ACL_SIZE_INFORMATION,
    DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
};
use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeClientProcessId,
    PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows_sys::Win32::System::Services::{
    RegisterServiceCtrlHandlerExW, SetServiceStatus, StartServiceCtrlDispatcherW,
    SERVICE_ACCEPT_STOP, SERVICE_CONTROL_STOP, SERVICE_RUNNING, SERVICE_START_PENDING,
    SERVICE_STATUS, SERVICE_STATUS_HANDLE, SERVICE_STOPPED, SERVICE_STOP_PENDING,
    SERVICE_TABLE_ENTRYW, SERVICE_WIN32_OWN_PROCESS,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};

pub(crate) const SERVICE_NAME: &str = "NeutrasearchHelper";
pub(crate) const PIPE_PATH: &str = r"\\.\pipe\Neutrasearch.Helper.v1";

static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);
static STATUS_HANDLE: AtomicPtr<c_void> = AtomicPtr::new(null_mut());
static ACTIVE_PIPE: AtomicPtr<c_void> = AtomicPtr::new(null_mut());

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
    report_status(SERVICE_START_PENDING, 0, 3_000);

    let result = service_body();
    if let Err(error) = &result {
        service_log(&format!("service stopped after error: {error:#}"));
    }
    let exit_code = i32::from(result.is_err());
    report_status(SERVICE_STOPPED, exit_code as u32, 0);
    STATUS_HANDLE.store(null_mut(), Ordering::Release);
    // This executable hosts exactly one SCM service. A stop disconnects the
    // protocol pipe, which releases the service thread immediately; exiting
    // the process then cancels any read-only native scan worker still parsing
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
        let pipe = ACTIVE_PIPE.load(Ordering::Acquire) as HANDLE;
        if !pipe.is_null() {
            unsafe {
                DisconnectNamedPipe(pipe);
            }
        }
        // Also connect to our own pipe to release a ConnectNamedPipe call that
        // raced with the active-handle disconnect above.
        let _ = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(PIPE_PATH);
    }
    0
}

fn report_status(state: u32, exit_code: u32, wait_hint: u32) {
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

fn service_body() -> Result<()> {
    init_service_logging()?;
    validate_installed_service_files()?;
    service_log("service starting");
    report_status(SERVICE_RUNNING, 0, 0);

    while !STOP_REQUESTED.load(Ordering::Acquire) {
        let pipe = create_pipe().context("create privileged scanner pipe")?;
        ACTIVE_PIPE.store(pipe.cast(), Ordering::Release);
        let connected = unsafe { ConnectNamedPipe(pipe, null_mut()) };
        if connected == 0 && unsafe { GetLastError() } != ERROR_PIPE_CONNECTED {
            ACTIVE_PIPE.store(null_mut(), Ordering::Release);
            unsafe { CloseHandle(pipe) };
            if STOP_REQUESTED.load(Ordering::Acquire) {
                break;
            }
            return Err(std::io::Error::last_os_error()).context("accept scanner pipe client");
        }
        if STOP_REQUESTED.load(Ordering::Acquire) {
            ACTIVE_PIPE.store(null_mut(), Ordering::Release);
            unsafe {
                DisconnectNamedPipe(pipe);
                CloseHandle(pipe);
            }
            break;
        }

        if let Err(error) = verify_client(pipe) {
            service_log(&format!("rejected pipe client: {error:#}"));
            ACTIVE_PIPE.store(null_mut(), Ordering::Release);
            unsafe {
                DisconnectNamedPipe(pipe);
                CloseHandle(pipe);
            }
            continue;
        }

        // File owns the pipe handle from here. A cloned handle lets the framed
        // protocol read and write concurrently while scan workers stream data.
        let mut reader = unsafe { File::from_raw_handle(pipe.cast()) };
        let writer = reader.try_clone().context("clone scanner pipe handle")?;
        if let Err(error) = super::run_protocol(
            &mut reader,
            Box::new(writer),
            None,
            None,
            Some(&STOP_REQUESTED),
        ) {
            if !STOP_REQUESTED.load(Ordering::Acquire) {
                service_log(&format!("client protocol ended with error: {error:#}"));
            }
        }
        ACTIVE_PIPE.store(null_mut(), Ordering::Release);
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

fn validate_installed_service_files() -> Result<()> {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    let helper = std::env::current_exe().context("resolve service executable")?;
    let directory = helper
        .parent()
        .context("service executable has no installation directory")?;
    let program_files = std::env::var_os("ProgramFiles")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .context("ProgramFiles is unavailable")?;
    if !windows_path_is_under(directory, &program_files) {
        anyhow::bail!(
            "privileged scanner is outside Program Files: {}",
            directory.display()
        );
    }

    let gui = directory.join("neutrasearch.exe");
    for file in [&helper, &gui] {
        let metadata = std::fs::symlink_metadata(file)
            .with_context(|| format!("inspect installed service file {}", file.display()))?;
        if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            anyhow::bail!(
                "privileged scanner file must be regular and not a reparse point: {}",
                file.display()
            );
        }
    }

    let mut ancestor = Some(directory);
    while let Some(path) = ancestor {
        let metadata = std::fs::symlink_metadata(path)
            .with_context(|| format!("inspect service directory {}", path.display()))?;
        if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            anyhow::bail!(
                "privileged scanner directory must not contain reparse points: {}",
                path.display()
            );
        }
        if windows_path_eq(path, &program_files) {
            break;
        }
        ancestor = path.parent();
    }
    for path in [directory, helper.as_path(), gui.as_path()] {
        reject_non_admin_write_acl(path)?;
    }
    Ok(())
}

fn reject_non_admin_write_acl(path: &Path) -> Result<()> {
    const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
    const WRITE_MASK: u32 = 0x0000_0002 // FILE_WRITE_DATA / FILE_ADD_FILE
        | 0x0000_0004 // FILE_APPEND_DATA / FILE_ADD_SUBDIRECTORY
        | 0x0000_0010 // FILE_WRITE_EA
        | 0x0000_0040 // FILE_DELETE_CHILD
        | 0x0000_0100 // FILE_WRITE_ATTRIBUTES
        | 0x0001_0000 // DELETE
        | 0x0004_0000 // WRITE_DAC
        | 0x0008_0000 // WRITE_OWNER
        | 0x1000_0000 // GENERIC_ALL
        | 0x4000_0000; // GENERIC_WRITE

    let name = wide(&path.to_string_lossy());
    let mut dacl = null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    let status = unsafe {
        GetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 {
        anyhow::bail!(
            "read ACL for {} failed with Windows error {status}",
            path.display()
        );
    }
    let result = (|| -> Result<()> {
        if dacl.is_null() {
            anyhow::bail!("refusing unprotected null DACL on {}", path.display());
        }
        let mut information = ACL_SIZE_INFORMATION::default();
        if unsafe {
            GetAclInformation(
                dacl,
                (&mut information as *mut ACL_SIZE_INFORMATION).cast(),
                std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
                AclSizeInformation,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error()).context("read ACL entry count");
        }
        for index in 0..information.AceCount {
            let mut raw_ace = null_mut();
            if unsafe { GetAce(dacl, index, &mut raw_ace) } == 0 || raw_ace.is_null() {
                return Err(std::io::Error::last_os_error()).context("read ACL entry");
            }
            let ace = unsafe { &*(raw_ace as *const ACCESS_ALLOWED_ACE) };
            // The installer emits only ordinary allow ACEs. Unknown/object or
            // callback ACEs are rejected rather than parsed optimistically.
            if ace.Header.AceType != ACCESS_ALLOWED_ACE_TYPE {
                anyhow::bail!("unexpected ACL entry type on {}", path.display());
            }
            if ace.Mask & WRITE_MASK == 0 {
                continue;
            }
            let sid = (&ace.SidStart as *const u32).cast_mut().cast();
            let sid = sid_string(sid)?;
            if !matches!(sid.as_str(), "S-1-5-18" | "S-1-5-32-544") {
                anyhow::bail!(
                    "non-administrator SID {sid} can modify privileged scanner path {}",
                    path.display()
                );
            }
        }
        Ok(())
    })();
    unsafe {
        LocalFree(descriptor);
    }
    result
}

fn sid_string(sid: windows_sys::Win32::Security::PSID) -> Result<String> {
    let mut text = null_mut();
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 || text.is_null() {
        return Err(std::io::Error::last_os_error()).context("render ACL SID");
    }
    let mut length = 0usize;
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    let value = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) });
    unsafe {
        LocalFree(text.cast());
    }
    Ok(value)
}

fn verify_client(pipe: HANDLE) -> Result<()> {
    let mut pid = 0u32;
    if unsafe { GetNamedPipeClientProcessId(pipe, &mut pid) } == 0 || pid == 0 {
        return Err(std::io::Error::last_os_error()).context("identify pipe client process");
    }
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return Err(std::io::Error::last_os_error()).context("open pipe client process");
    }
    let result = process_image(process);
    unsafe {
        CloseHandle(process);
    }
    let client = result?;
    let helper = std::env::current_exe().context("resolve installed helper path")?;
    validate_client_path(&helper, &client)
}

fn process_image(process: HANDLE) -> Result<PathBuf> {
    let mut buffer = vec![0u16; 32_768];
    let mut length = buffer.len() as u32;
    if unsafe { QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut length) } == 0 {
        return Err(std::io::Error::last_os_error()).context("read pipe client executable path");
    }
    buffer.truncate(length as usize);
    Ok(PathBuf::from(String::from_utf16_lossy(&buffer)))
}

fn validate_client_path(helper: &Path, client: &Path) -> Result<()> {
    let expected_directory = helper
        .parent()
        .context("installed helper has no parent directory")?;
    let client_directory = client
        .parent()
        .context("pipe client has no parent directory")?;
    let filename = client
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or_default();
    if !filename.eq_ignore_ascii_case("neutrasearch.exe")
        || !windows_path_eq(expected_directory, client_directory)
    {
        anyhow::bail!(
            "client must be the installed Neutrasearch GUI beside the service helper (got {})",
            client.display()
        );
    }
    Ok(())
}

fn windows_path_eq(left: &Path, right: &Path) -> bool {
    normalize_windows_path(left) == normalize_windows_path(right)
}

fn windows_path_is_under(path: &Path, root: &Path) -> bool {
    let path = normalize_windows_path(path);
    let root = normalize_windows_path(root);
    path.strip_prefix(&root)
        .is_some_and(|tail| tail.starts_with('\\'))
}

fn normalize_windows_path(path: &Path) -> String {
    path.to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

fn init_service_logging() -> Result<()> {
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

fn service_log(message: &str) {
    tracing::info!(target: "neutra_helper::windows_service", "{message}");
}

fn wide(value: &str) -> Vec<u16> {
    OsStr::new(value).encode_wide().chain(Some(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_installed_gui_beside_the_helper_is_trusted() {
        let helper = Path::new(r"C:\Program Files\Neutrasearch\neutrasearch-helper.exe");
        assert!(validate_client_path(
            helper,
            Path::new(r"c:\program files\neutrasearch\Neutrasearch.exe")
        )
        .is_ok());
        assert!(validate_client_path(helper, Path::new(r"C:\Temp\neutrasearch.exe")).is_err());
        assert!(validate_client_path(
            helper,
            Path::new(r"C:\Program Files\Neutrasearch\neutrasearch-mcp.exe")
        )
        .is_err());
        assert!(windows_path_is_under(
            Path::new(r"C:\Program Files\Neutrasearch"),
            Path::new(r"C:\Program Files")
        ));
        assert!(!windows_path_is_under(
            Path::new(r"C:\Program Files-old\Neutrasearch"),
            Path::new(r"C:\Program Files")
        ));
    }
}
