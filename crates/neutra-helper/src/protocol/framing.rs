use super::*;

pub(crate) fn run_protocol<R: Read>(
    rin: &mut R,
    writer: Box<dyn Write + Send>,
    serve_index: Option<std::path::PathBuf>,
    watch_mount: Option<(std::path::PathBuf, u32)>,
    stop_requested: Option<&AtomicBool>,
) -> Result<()> {
    run_protocol_with_auth(rin, writer, serve_index, watch_mount, stop_requested, None)
}

pub fn run_protocol_with_auth<R: Read>(
    rin: &mut R,
    writer: Box<dyn Write + Send>,
    serve_index: Option<std::path::PathBuf>,
    watch_mount: Option<(std::path::PathBuf, u32)>,
    stop_requested: Option<&AtomicBool>,
    authenticate: Option<&dyn Fn() -> Result<()>>,
) -> Result<()> {
    let out: ProtocolOutput = Arc::new(Mutex::new(BufWriter::new(writer)));

    if !handshake::authenticate(rin, &out, authenticate)? {
        return Ok(());
    }

    let mut session = state::Session::open(out, serve_index, watch_mount)?;
    tracing::info!(target: "neutra_helper::protocol", "waiting for command frame");
    loop {
        let frame = read_frame(rin);
        if stop_requested.is_some_and(|stop| stop.load(Ordering::Acquire)) {
            // Windows service shutdown disconnects the pipe. Return without
            // joining read-only scan workers; the single-service process exits
            // immediately and the GUI discards its unpublished staging index.
            return Ok(());
        }
        let msg: Option<ClientMsg> = frame.context("reading command")?;
        tracing::info!(target: "neutra_helper::protocol", received = msg.is_some(), "command frame read");
        reap_scan_threads(&mut session.scan_threads);
        match dispatch::handle(msg, &mut session, stop_requested.is_some())? {
            dispatch::Flow::Continue => {}
            dispatch::Flow::Shutdown => break,
            dispatch::Flow::Disconnect => return Ok(()),
        }
    }

    for t in session.scan_threads {
        let _ = t.join();
    }
    Ok(())
}
