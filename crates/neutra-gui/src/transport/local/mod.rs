use super::*;
mod cancel;
mod stderr;

pub(crate) fn spawn_local_helper(
    tx: EventSender,
    elevated_requested: bool,
    mounts: Vec<MountInfo>,
    roots: Vec<PathBuf>,
    allow_zfs_enumerate: bool,
) {
    spawn_cancellable_local_helper(
        tx,
        elevated_requested,
        mounts,
        roots,
        allow_zfs_enumerate,
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
    );
}

pub(crate) fn spawn_cancellable_local_helper(
    tx: EventSender,
    elevated_requested: bool,
    mounts: Vec<MountInfo>,
    roots: Vec<PathBuf>,
    allow_zfs_enumerate: bool,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
) {
    std::thread::spawn(move || {
        if let Err(error) = run(
            &tx,
            elevated_requested,
            mounts,
            roots,
            allow_zfs_enumerate,
            &cancelled,
        ) {
            let _ = tx.send(Event::Fatal(error));
        }
    });
}

fn run(
    tx: &EventSender,
    elevated: bool,
    mounts: Vec<MountInfo>,
    roots: Vec<PathBuf>,
    allow_zfs_enumerate: bool,
    cancelled: &Arc<std::sync::atomic::AtomicBool>,
) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    if scan_via_windows_service(tx.clone(), mounts.clone(), roots.clone())? {
        return Ok(());
    }
    let mut command = scan_command(elevated)?;
    if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
        return Err("Indexing cancelled".into());
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            format!("Cannot start indexing: {error}. Repair indexing access in Index settings.")
        })?;
    let cancellation = cancel::Guard::new(child.id(), Arc::clone(cancelled));
    let capture = stderr::Capture::start(child.stderr.take());
    let result = exchange(
        &mut child,
        tx,
        ClientMsg::Scan {
            mounts,
            roots,
            allow_zfs_enumerate,
        },
    );
    if result.is_err() {
        let _ = child.kill();
    }
    drop(cancellation);
    let status = child.wait().map_err(|error| error.to_string());
    let text = capture.finish();
    match result {
        Err(error) => Err(helper_start_failure(&error, &text)),
        Ok(completion) => {
            let status = status?;
            if !status.success() || completion.is_none() {
                return Err(helper_start_failure(
                    &format!("The scanner stopped before completing ({status})"),
                    &text,
                ));
            }
            let _ = tx.send(Event::Message(completion.unwrap()));
            Ok(())
        }
    }
}

fn scan_command(elevated_requested: bool) -> Result<Command, String> {
    let configured = std::env::var_os("NEUTRASEARCH_HELPER").map(PathBuf::from);
    let elevated = cfg!(target_os = "linux")
        && (elevated_requested || std::env::var_os("NEUTRASEARCH_PKEXEC").is_some());
    #[cfg(target_os = "linux")]
    if configured.is_none() && std::env::var_os("NEUTRASEARCH_INDEX").is_none() {
        if let Some(helper) = indexing_access::installed_helper() {
            indexing_access::authorize(&helper)?;
            indexing_access::control_watcher("stop")?;
            return Ok(Command::new(helper));
        }
    }
    select_helper(configured, std::env::current_exe().ok(), elevated).map(Command::new)
}

fn exchange(
    child: &mut std::process::Child,
    tx: &EventSender,
    scan: ClientMsg,
) -> Result<Option<HelperMsg>, String> {
    let stdin = child
        .stdin
        .take()
        .ok_or("The scanner input pipe is unavailable")?;
    let stdout = child
        .stdout
        .take()
        .ok_or("The scanner output pipe is unavailable")?;
    let (mut input, mut output) = (BufWriter::new(stdin), BufReader::new(stdout));
    write_frame(
        &mut input,
        &ClientMsg::Hello {
            proto: PROTO_VERSION,
        },
    )
    .map_err(|error| format!("Cannot contact the scanner: {error}"))?;
    match read_frame::<_, HelperMsg>(&mut output).map_err(|error| error.to_string())? {
        Some(
            message @ HelperMsg::Hello {
                proto: PROTO_VERSION,
                ..
            },
        ) => {
            let _ = tx.send(Event::Message(message));
        }
        other => return Err(format!("Scanner handshake failed: {other:?}")),
    }
    write_frame(&mut input, &scan)
        .map_err(|error| format!("Cannot send search locations: {error}"))?;
    drop(input);
    let mut completed = None;
    while let Some(message) = read_frame::<_, HelperMsg>(&mut output)
        .map_err(|error| format!("Scanner protocol: {error}"))?
    {
        if matches!(message, HelperMsg::ScanComplete { .. }) {
            completed = Some(message);
        } else {
            let _ = tx.send(Event::Message(message));
        }
    }
    Ok(completed)
}
