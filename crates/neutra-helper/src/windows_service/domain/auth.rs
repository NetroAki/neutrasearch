use super::super::state::service_log;
use super::validate_client_path;
use anyhow::{Context, Result};
use std::{
    path::PathBuf,
    sync::mpsc::{self, RecvTimeoutError},
    time::Duration,
};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::Pipes::GetNamedPipeClientProcessId;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::System::Threading::{
    OpenProcess, OpenThread, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    THREAD_TERMINATE,
};
use windows_sys::Win32::System::IO::CancelSynchronousIo;

pub(super) const CLIENT_AUTH_TIMEOUT: Duration = Duration::from_secs(2);

enum AuthEvent {
    WorkerThread(usize),
    Finished(Result<PathBuf>),
}

pub(in crate::windows_service) fn verify_client(pipe: HANDLE) -> Result<()> {
    service_log("client auth: querying named-pipe client PID");
    let mut pid = 0u32;
    if unsafe { GetNamedPipeClientProcessId(pipe, &mut pid) } == 0 || pid == 0 {
        return Err(std::io::Error::last_os_error()).context("identify pipe client process");
    }
    service_log(&format!(
        "client auth: identified PID {pid}; starting image query worker"
    ));
    // Process-image inspection is a synchronous kernel call. Keep it off the
    // sole service accept thread and cancel it on a deadline so a bad client
    // cannot prevent the next named-pipe client from connecting.
    let (events_tx, events_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let thread = unsafe { OpenThread(THREAD_TERMINATE, 0, GetCurrentThreadId()) };
        if thread.is_null() {
            let _ = events_tx.send(AuthEvent::Finished(Err(
                std::io::Error::last_os_error().into()
            )));
            return;
        }
        if events_tx
            .send(AuthEvent::WorkerThread(thread as usize))
            .is_err()
        {
            unsafe { CloseHandle(thread) };
            return;
        }
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        let result = if process.is_null() {
            Err(anyhow::anyhow!(std::io::Error::last_os_error()))
        } else {
            let result = process_image(process);
            unsafe { CloseHandle(process) };
            result
        };
        let _ = events_tx.send(AuthEvent::Finished(result));
    });

    let mut worker_thread = None;
    loop {
        match events_rx.recv_timeout(CLIENT_AUTH_TIMEOUT) {
            Ok(AuthEvent::WorkerThread(handle)) => {
                service_log("client auth: image query worker started");
                worker_thread = Some(handle)
            }
            Ok(AuthEvent::Finished(result)) => {
                service_log("client auth: image query worker completed");
                if let Some(handle) = worker_thread {
                    unsafe { CloseHandle(handle as HANDLE) };
                }
                let _ = worker.join();
                let client = result.context("read pipe client executable path")?;
                let helper = std::env::current_exe().context("resolve installed helper path")?;
                return validate_client_path(&helper, &client);
            }
            Err(RecvTimeoutError::Timeout) => {
                service_log("client auth: image query timed out after 2 seconds");
                if let Some(handle) = worker_thread.or_else(|| match events_rx.try_recv() {
                    Ok(AuthEvent::WorkerThread(handle)) => Some(handle),
                    _ => None,
                }) {
                    unsafe {
                        CancelSynchronousIo(handle as HANDLE);
                        CloseHandle(handle as HANDLE);
                    }
                }
                // Detach after cancellation: accepting the next client must not
                // depend on Windows interrupting a pathological kernel call.
                drop(worker);
                anyhow::bail!("pipe client authentication timed out after 2 seconds");
            }
            Err(RecvTimeoutError::Disconnected) => {
                service_log("client auth: image query worker channel disconnected");
                let _ = worker.join();
                anyhow::bail!("pipe client authentication worker stopped unexpectedly");
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_authentication_has_a_bounded_deadline() {
        assert!(CLIENT_AUTH_TIMEOUT <= Duration::from_secs(2));
        assert!(!CLIENT_AUTH_TIMEOUT.is_zero());
    }
}
