//! Out-of-process transports for the GUI: the local helper child (or the
//! Windows service pipe), elevated helper selection, network-helper
//! provisioning, and file open/reveal actions. Split from `main` so UI state
//! stays separate from process management.

use crate::Event;
use neutra_core::proto::{read_frame, write_frame, ClientMsg, HelperMsg, PROTO_VERSION};
use neutra_core::MountInfo;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, BufWriter};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub(crate) enum FileAction {
    Open(PathBuf),
    Reveal(PathBuf),
}

#[cfg(target_os = "windows")]
fn request_elevated_restart() -> Result<(), String> {
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

pub(crate) fn launch_file_action(action: FileAction) -> std::io::Result<()> {
    let mut command = match action {
        FileAction::Open(path) => {
            #[cfg(target_os = "windows")]
            {
                let mut command = Command::new("explorer.exe");
                command.arg(path);
                command
            }
            #[cfg(target_os = "macos")]
            {
                let mut command = Command::new("open");
                command.arg(path);
                command
            }
            #[cfg(not(any(target_os = "windows", target_os = "macos")))]
            {
                let mut command = Command::new("xdg-open");
                command.arg(path);
                command
            }
        }
        FileAction::Reveal(path) => {
            #[cfg(target_os = "windows")]
            {
                let mut command = Command::new("explorer.exe");
                command.arg(format!("/select,{}", path.display()));
                command
            }
            #[cfg(target_os = "macos")]
            {
                let mut command = Command::new("open");
                command.arg("-R").arg(path);
                command
            }
            #[cfg(not(any(target_os = "windows", target_os = "macos")))]
            {
                let mut command = Command::new("xdg-open");
                command.arg(path.parent().unwrap_or(&path));
                command
            }
        }
    };
    command.spawn()?;
    Ok(())
}

pub(crate) fn select_helper(
    configured: Option<PathBuf>,
    current_exe: Option<PathBuf>,
    elevated: bool,
) -> Result<PathBuf, String> {
    if elevated && configured.is_some() {
        return Err(
            "refusing to elevate a helper selected through NEUTRASEARCH_HELPER; install a trusted system helper"
                .into(),
        );
    }
    let sibling = current_exe.map(|path| {
        path.with_file_name(if cfg!(windows) {
            "neutrasearch-helper.exe"
        } else {
            "neutrasearch-helper"
        })
    });
    if elevated {
        #[cfg(unix)]
        let candidates = [
            PathBuf::from("/usr/local/lib/neutrasearch/neutrasearch-helper"),
            PathBuf::from("/usr/lib/neutrasearch/neutrasearch-helper"),
            PathBuf::from("/usr/local/bin/neutrasearch-helper"),
        ];
        #[cfg(not(unix))]
        let candidates = sibling.into_iter().collect::<Vec<_>>();
        for helper in candidates {
            if let Ok(helper) = validate_elevated_helper(&helper) {
                return Ok(helper);
            }
        }
        return Err(
            "no trusted administrator helper is installed; reinstall Neutrasearch system-wide"
                .into(),
        );
    }
    Ok(configured
        .or(sibling)
        .unwrap_or_else(|| PathBuf::from("neutrasearch-helper")))
}

#[cfg(unix)]
pub(crate) fn validate_elevated_helper(path: &std::path::Path) -> Result<PathBuf, String> {
    use std::os::unix::fs::MetadataExt;
    let path = std::fs::canonicalize(path).map_err(|error| {
        format!(
            "cannot resolve installed helper {}: {error}",
            path.display()
        )
    })?;
    let allowed = [
        std::path::Path::new("/usr/local/lib/neutrasearch"),
        std::path::Path::new("/usr/lib/neutrasearch"),
        std::path::Path::new("/usr/local/bin"),
    ];
    if !allowed.iter().any(|directory| path.starts_with(directory)) {
        return Err(format!(
            "refusing helper outside trusted system locations: {}",
            path.display()
        ));
    }
    let metadata = std::fs::metadata(&path).map_err(|error| {
        format!(
            "cannot inspect installed helper {}: {error}",
            path.display()
        )
    })?;
    if !metadata.is_file() || metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
        return Err(format!(
            "refusing to elevate untrusted helper {}; it must be a root-owned regular file not writable by group/others",
            path.display()
        ));
    }
    let mut ancestor = path.parent();
    while let Some(directory) = ancestor {
        let metadata = std::fs::metadata(directory).map_err(|error| {
            format!(
                "cannot inspect helper directory {}: {error}",
                directory.display()
            )
        })?;
        if !metadata.is_dir() || metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
            return Err(format!(
                "refusing helper beneath untrusted directory {}",
                directory.display()
            ));
        }
        ancestor = directory.parent();
    }
    Ok(path)
}

pub(crate) fn spawn_local_helper(
    tx: Sender<Event>,
    elevated_requested: bool,
    mounts: Vec<MountInfo>,
    roots: Vec<PathBuf>,
    allow_zfs_enumerate: bool,
) {
    std::thread::spawn(move || {
        #[cfg(target_os = "windows")]
        match scan_via_windows_service(tx.clone(), mounts.clone(), roots.clone()) {
            Ok(true) => return,
            Ok(false) => {
                // Portable archives do not install the service. Preserve their
                // sibling-helper path (and the existing explicit elevation UX).
            }
            Err(error) => {
                let _ = tx.send(Event::Fatal(error));
                return;
            }
        }

         let configured = std::env::var_os("NEUTRASEARCH_HELPER").map(PathBuf::from);
         let elevated = cfg!(target_os = "linux")
             && (elevated_requested || std::env::var_os("NEUTRASEARCH_PKEXEC").is_some());
        let helper = match select_helper(configured, std::env::current_exe().ok(), elevated) {
            Ok(helper) => helper,
            Err(error) => {
                let _ = tx.send(Event::Fatal(error));
                return;
            }
        };
        let mut cmd = if elevated {
            let mut command = Command::new("pkexec");
            command.arg(helper);
            command
        } else {
            Command::new(helper)
        };
        let child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match child {
            Ok(child) => child,
            Err(error) => {
                let _ = tx.send(Event::Fatal(format!(
                    "cannot start the native scanner: {error}; install neutrasearch-helper beside the Neutrasearch executable (or point NEUTRASEARCH_HELPER at it)"
                )));
                return;
            }
        };
        let stderr_text = Arc::new(Mutex::new(String::new()));
        let stderr_reader = child.stderr.take().map(|stderr| {
            let captured = Arc::clone(&stderr_text);
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    eprintln!("neutrasearch-helper: {line}");
                    if let Ok(mut text) = captured.lock() {
                        if text.len() < 8_192 {
                            if !text.is_empty() {
                                text.push('\n');
                            }
                            text.push_str(&line);
                            text.truncate(8_192);
                        }
                    }
                }
            })
        });
        let Some(stdin) = child.stdin.take() else {
            let _ = child.kill();
            let _ = child.wait();
            if let Some(reader) = stderr_reader {
                let _ = reader.join();
            }
            let _ = tx.send(Event::Fatal(helper_start_failure(
                "native scanner input pipe is unavailable",
                &stderr_text,
            )));
            return;
        };
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            if let Some(reader) = stderr_reader {
                let _ = reader.join();
            }
            let _ = tx.send(Event::Fatal(helper_start_failure(
                "native scanner output pipe is unavailable",
                &stderr_text,
            )));
            return;
        };
        let (mut input, mut output) = (BufWriter::new(stdin), BufReader::new(stdout));
        if let Err(error) = write_frame(
            &mut input,
            &ClientMsg::Hello {
                proto: PROTO_VERSION,
            },
        ) {
            let _ = child.wait();
            if let Some(reader) = stderr_reader {
                let _ = reader.join();
            }
            let _ = tx.send(Event::Fatal(helper_start_failure(
                &format!("cannot contact the native scanner: {error}"),
                &stderr_text,
            )));
            return;
        }
        match read_frame::<_, HelperMsg>(&mut output) {
            Ok(Some(message)) => {
                let _ = tx.send(Event::Message(message));
            }
            other => {
                let _ = child.wait();
                if let Some(reader) = stderr_reader {
                    let _ = reader.join();
                }
                let _ = tx.send(Event::Fatal(helper_start_failure(
                    &format!("native scanner handshake failed: {other:?}"),
                    &stderr_text,
                )));
                return;
            }
        }
        if let Err(error) = write_frame(
            &mut input,
            &ClientMsg::Scan {
                mounts,
                roots,
                allow_zfs_enumerate,
            },
        ) {
            drop(input);
            let _ = child.wait();
            if let Some(reader) = stderr_reader {
                let _ = reader.join();
            }
            let _ = tx.send(Event::Fatal(helper_start_failure(
                &format!("cannot send locations to the native scanner: {error}"),
                &stderr_text,
            )));
            return;
        }
        drop(input);
        let mut completed = false;
        loop {
            match read_frame::<_, HelperMsg>(&mut output) {
                Ok(Some(message)) => {
                    completed |= matches!(message, HelperMsg::ScanComplete { .. });
                    let _ = tx.send(Event::Message(message));
                }
                Ok(None) => break,
                Err(error) => {
                    let _ = tx.send(Event::Fatal(format!("native scanner protocol: {error}")));
                    break;
                }
            }
        }
        let status = child.wait().ok();
        if let Some(reader) = stderr_reader {
            let _ = reader.join();
        }
        if !completed {
            let status_detail = status
                .map(|status| format!(" ({status})"))
                .unwrap_or_default();
            let _ = tx.send(Event::Fatal(helper_start_failure(
                &format!("the native scanner stopped before completing{status_detail}"),
                &stderr_text,
            )));
        }
    });
}

#[cfg(target_os = "windows")]
fn scan_via_windows_service(
    tx: Sender<Event>,
    mounts: Vec<MountInfo>,
    roots: Vec<PathBuf>,
) -> Result<bool, String> {
    const PIPE: &str = r"\\.\pipe\Neutrasearch.Helper.v1";
    let deadline = Instant::now() + Duration::from_secs(5);
    let pipe = loop {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(PIPE)
        {
            Ok(pipe) => break pipe,
            Err(error)
                if Instant::now() < deadline && matches!(error.raw_os_error(), Some(2 | 231)) =>
            {
                std::thread::sleep(Duration::from_millis(125));
            }
            Err(error) if error.raw_os_error() == Some(2) => {
                if windows_scanner_service_installed() {
                    return Err(
                        "the installed scanner service is unavailable; repair or restart the NeutrasearchHelper service"
                            .into(),
                    );
                }
                return Ok(false);
            }
            Err(error) if error.raw_os_error() == Some(231) => {
                return Err(
                    "the installed scanner service is busy with another scan; wait a moment and try again"
                        .into(),
                );
            }
            Err(error) => {
                return Err(format!(
                    "cannot connect to the installed scanner service: {error}; repair the Neutrasearch installation"
                ));
            }
        }
    };
    let writer = pipe
        .try_clone()
        .map_err(|error| format!("cannot clone the scanner service pipe: {error}"))?;
    let mut input = BufWriter::new(writer);
    let mut output = BufReader::new(pipe);

    // The service is the authority for this pipe: startup validates both installed
    // binaries under Program Files, the named-pipe ACL excludes remote clients,
    // and the service authenticates this GUI executable after Hello and before
    // accepting Scan/Search. Do not inspect the server image here: doing so would
    // synchronously inspect the service while it synchronously inspects this
    // process, recreating the cross-authentication deadlock.
    eprintln!("neutrasearch: service Hello write");
    write_frame(
        &mut input,
        &ClientMsg::Hello {
            proto: PROTO_VERSION,
        },
    )
    .map_err(|error| format!("cannot contact the installed scanner service: {error}"))?;
    eprintln!("neutrasearch: service Hello read");
    let hello: Option<HelperMsg> = match read_frame(&mut output) {
        Ok(hello) => hello,
        Err(error) => {
            let _ = write_frame(&mut input, &ClientMsg::Shutdown);
            return Err(format!("scanner service handshake failed: {error}"));
        }
    };
    let hello = match hello {
        Some(message @ HelperMsg::Hello { proto, .. }) if proto == PROTO_VERSION => message,
        Some(HelperMsg::Hello { proto, .. }) => {
            let _ = write_frame(&mut input, &ClientMsg::Shutdown);
            return Err(format!(
                "scanner service protocol mismatch: GUI={PROTO_VERSION}, service={proto}; repair the installation"
            ));
        }
        Some(message) => {
            let _ = write_frame(&mut input, &ClientMsg::Shutdown);
            return Err(format!(
                "scanner service returned an invalid handshake: {message:?}"
            ));
        }
        None => return Err("the installed scanner service closed during handshake".into()),
    };
    let _ = tx.send(Event::Message(hello));

    eprintln!("neutrasearch: service Scan write");
    if let Err(error) = write_frame(
            &mut input,
            &ClientMsg::Scan {
                mounts,
                roots,
                allow_zfs_enumerate: false,
            },
        ) {
        let _ = write_frame(&mut input, &ClientMsg::Shutdown);
        return Err(format!(
            "cannot send locations to the scanner service: {error}"
        ));
    }

    let mut completed = false;
    loop {
        eprintln!("neutrasearch: service response read");
        match read_frame::<_, HelperMsg>(&mut output) {
            Ok(Some(message)) => {
                let terminal_error = matches!(message, HelperMsg::Error(_));
                completed |= matches!(message, HelperMsg::ScanComplete { .. });
                let _ = tx.send(Event::Message(message));
                if terminal_error {
                    // A service Error ends this one-shot client session. The
                    // service itself remains alive and accepts the next client.
                    let _ = write_frame(&mut input, &ClientMsg::Shutdown);
                    return Ok(true);
                }
                if completed {
                    break;
                }
            }
            Ok(None) => break,
            Err(error) => {
                let _ = write_frame(&mut input, &ClientMsg::Shutdown);
                return Err(format!("scanner service protocol failed: {error}"));
            }
        }
    }
    if !completed {
        let _ = write_frame(&mut input, &ClientMsg::Shutdown);
        return Err("the installed scanner service stopped before completing".into());
    }
    // End this per-client service session explicitly; the service remains
    // running and accepts the next ordinary-user scan without another UAC.
    let _ = write_frame(&mut input, &ClientMsg::Shutdown);
    Ok(true)
}

#[cfg(target_os = "windows")]
fn windows_scanner_service_installed() -> bool {
    let sc = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("System32/sc.exe");
    Command::new(sc)
        .args(["query", "NeutrasearchHelper"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

pub(crate) fn helper_start_failure(summary: &str, stderr: &Arc<Mutex<String>>) -> String {
    let detail = stderr
        .lock()
        .map(|text| text.trim().to_owned())
        .unwrap_or_default();
    if detail.contains("textual authentication agent")
        || detail.contains("current controlling terminal")
    {
        return "administrator approval could not open in this desktop session; launch Neutrasearch from the desktop and try again".into();
    }
    if detail.is_empty() {
        summary.to_owned()
    } else {
        format!("{summary}: {detail}")
    }
}

pub(crate) fn spawn_network_watcher(tx: Sender<Event>) {
    std::thread::spawn(move || {
        let provisioner = neutra_remote::Provisioner::from_env();
        let mut ready = std::collections::HashSet::<String>::new();
        let mut last_attempt = BTreeMap::<String, Instant>::new();
        let mut announced_waiting = std::collections::HashSet::<String>::new();
        loop {
            for (host, key) in discover_network_hosts() {
                if ready.contains(&key)
                    || last_attempt
                        .get(&key)
                        .is_some_and(|attempt| attempt.elapsed() < Duration::from_secs(30))
                {
                    continue;
                }
                last_attempt.insert(key.clone(), Instant::now());
                match provisioner.ensure_installed(&host) {
                    Ok(platform) => {
                        ready.insert(key.clone());
                        announced_waiting.remove(&key);
                        let _ = tx.send(Event::Remote {
                            key,
                            status: format!(
                                "helper build {} ready ({:?}/{})",
                                neutra_core::proto::HELPER_BUILD,
                                platform.os,
                                platform.arch
                            ),
                            error: false,
                        });
                    }
                    Err(error) if remote_failure_is_offline(&error) => {
                        if announced_waiting.insert(key.clone()) {
                            let _ = tx.send(Event::Remote {
                                key,
                                status: "offline; waiting to retry when the server is available"
                                    .into(),
                                error: false,
                            });
                        }
                    }
                    Err(error) => {
                        ready.insert(key.clone());
                        let _ = tx.send(Event::Remote {
                            key,
                            status: format!("network helper needs attention: {error:#}"),
                            error: true,
                        });
                    }
                }
            }
            std::thread::sleep(Duration::from_secs(3));
        }
    });
}

pub(crate) fn scan_has_reachable_lane(mounts: u32, errors: u32) -> bool {
    mounts > 0 && errors < mounts
}

pub(crate) fn remote_failure_is_offline(error: &anyhow::Error) -> bool {
    let message = format!("{error:#}").to_ascii_lowercase();
    [
        "connection timed out",
        "connection refused",
        "no route to host",
        "network is unreachable",
        "could not resolve hostname",
        "name or service not known",
        "operation timed out",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

fn discover_network_hosts() -> Vec<(String, String)> {
    #[cfg(target_os = "linux")]
    {
        return neutra_core::mounts::system_mounts()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|m| {
                m.network_host()
                    .map(|h| (h, format!("{}:{}", m.device, m.mountpoint.display())))
            })
            .collect();
    }
    #[cfg(target_os = "macos")]
    {
        let out = Command::new("/sbin/mount").output().ok();
        return out
            .into_iter()
            .flat_map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .filter_map(|l| {
                let (spec, rest) = l.split_once(" on ")?;
                if !(rest.contains("nfs") || rest.contains("smbfs") || rest.contains("webdav")) {
                    return None;
                }
                let host = spec
                    .trim_start_matches("//")
                    .rsplit('@')
                    .next()?
                    .split([':', '/'])
                    .next()?
                    .to_string();
                Some((host, l))
            })
            .collect();
    }
    #[cfg(target_os = "windows")]
    {
        let out = Command::new("net").arg("use").output().ok();
        return out
            .into_iter()
            .flat_map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .filter_map(|l| {
                let unc = l.split_whitespace().find(|s| s.starts_with(r"\\"))?;
                let host = unc.trim_start_matches('\\').split('\\').next()?.to_string();
                Some((host, l))
            })
            .collect();
    }
    #[allow(unreachable_code)]
    Vec::new()
}
