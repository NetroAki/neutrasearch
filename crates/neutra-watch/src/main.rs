//! Parked watch supervisor: completes the helper Hello handshake for
//! `--watch-index`, then holds the pipes open indefinitely. The helper watch
//! thread keeps committing deltas with no interactive client; if the helper
//! dies the read end hits EOF and this process exits so systemd restarts the
//! pair, and if this process dies the helper sees EOF and exits the same way.

mod health;
#[cfg(target_os = "linux")]
mod resume;
mod startup;

use neutra_core::proto::{read_frame, write_frame, ClientMsg, HelperMsg, PROTO_VERSION};
use std::io::{BufReader, BufWriter, Read, Write};
use std::process::{Command, Stdio};

fn usage() -> ! {
    eprintln!("usage: neutrasearch-watch MOUNT [SOURCE]");
    eprintln!("The index resolves exactly as the GUI and CLI resolve it.");
    std::process::exit(2);
}

fn helper_program() -> std::path::PathBuf {
    if let Some(path) = std::env::var_os("NEUTRASEARCH_HELPER") {
        return path.into();
    }
    if let Ok(current) = std::env::current_exe() {
        let candidate = current.with_file_name("neutrasearch-helper");
        if candidate.is_file() {
            return candidate;
        }
    }
    "neutrasearch-helper".into()
}

fn fail(message: String) -> ! {
    eprintln!("neutrasearch-watch: {message}");
    std::process::exit(1);
}

fn main() {
    #[cfg(target_os = "linux")]
    if std::env::args().nth(1).as_deref() == Some("--resume-after") {
        resume::after_parent_exit();
        return;
    }
    let mut args = std::env::args_os().skip(1);
    let (Some(mount), source) = (args.next(), args.next()) else {
        usage();
    };
    let source = source
        .map(|value| value.to_string_lossy().into_owned().parse().unwrap_or(0))
        .unwrap_or(0);
    let index = neutra_core::paths::resolve_index_path(None);
    startup::ensure_index(&index);
    let mut child = Command::new(helper_program())
        .arg("--watch-index")
        .arg(&index)
        .arg(&mount)
        .arg(source.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap_or_else(|error| fail(format!("cannot start helper: {error}")));
    let mut stdin = BufWriter::new(
        child
            .stdin
            .take()
            .unwrap_or_else(|| fail("no helper stdin".into())),
    );
    let mut stdout = BufReader::new(
        child
            .stdout
            .take()
            .unwrap_or_else(|| fail("no helper stdout".into())),
    );
    write_frame(
        &mut stdin,
        &ClientMsg::Hello {
            proto: PROTO_VERSION,
        },
    )
    .unwrap_or_else(|error| fail(format!("cannot greet helper: {error}")));
    match read_frame(&mut stdout)
        .unwrap_or_else(|error| fail(format!("no helper greeting: {error}")))
    {
        Some(HelperMsg::Hello { proto, .. }) if proto == PROTO_VERSION => {}
        other => fail(format!("helper handshake mismatch: {other:?}")),
    }
    match read_frame(&mut stdout)
        .unwrap_or_else(|error| fail(format!("cannot wait for native watches: {error}")))
    {
        Some(HelperMsg::WatchReady) => {}
        other => fail(format!("native watches failed to initialize: {other:?}")),
    }
    notify_service();
    health::monitor(&index);
    println!("ready");
    std::io::stdout()
        .flush()
        .unwrap_or_else(|error| fail(format!("cannot signal readiness: {error}")));
    eprintln!(
        "neutrasearch-watch: supervising {} (index {})",
        mount.to_string_lossy(),
        index.display()
    );
    let mut idle = [0u8; 1024];
    loop {
        match stdout.read(&mut idle) {
            Ok(0) => {
                if child.wait().is_ok_and(|status| status.code() == Some(78)) {
                    std::process::exit(78);
                }
                fail("helper exited".into())
            }
            Ok(_) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                fail(format!("helper pipe failed: {error}"))
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn notify_service() {
    if std::env::var_os("NOTIFY_SOCKET").is_none() {
        return;
    }
    let status = Command::new("/usr/bin/timeout")
        .args([
            "5",
            "/usr/bin/systemd-notify",
            "--ready",
            "--status=Native filesystem watchers attached",
        ])
        .status();
    if !status.is_ok_and(|status| status.success()) {
        fail("Cannot report native watcher readiness to the service manager".into());
    }
}

#[cfg(not(target_os = "linux"))]
fn notify_service() {}
