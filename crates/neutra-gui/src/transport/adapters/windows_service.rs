use crate::app::EventSender;
use crate::Event;
use neutra_core::proto::{read_frame, write_frame, ClientMsg, HelperMsg, PROTO_VERSION};
use neutra_core::MountInfo;
use std::io::{BufReader, BufWriter};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[cfg(target_os = "windows")]
pub(crate) fn scan_via_windows_service(
    tx: EventSender,
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
