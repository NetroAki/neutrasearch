use super::*;

pub(super) fn authenticate<R: Read>(
    rin: &mut R,
    out: &ProtocolOutput,
    authenticate: Option<&dyn Fn() -> Result<()>>,
) -> Result<bool> {
    tracing::info!(target: "neutra_helper::protocol", "waiting for client Hello");
    // Expect Hello first.
    let hello: Option<ClientMsg> = read_frame(rin).context("reading Hello")?;
    tracing::info!(target: "neutra_helper::protocol", received = hello.is_some(), "client Hello read");
    match hello {
        Some(ClientMsg::Hello { proto }) if proto == PROTO_VERSION => {
            send(
                out,
                &HelperMsg::Hello {
                    proto: PROTO_VERSION,
                    build: HELPER_BUILD,
                    os: std::env::consts::OS.to_string(),
                    arch: std::env::consts::ARCH.to_string(),
                },
            )?;
            tracing::info!(target: "neutra_helper::protocol", "helper Hello sent; starting client authentication");
            // The client needs the Hello response before image verification can
            // complete on Windows. Authentication still happens before any
            // command is read or executed.
            if let Some(authenticate) = authenticate {
                authenticate().context("authenticate pipe client")?;
                tracing::info!(target: "neutra_helper::protocol", "client authentication completed");
            }
            tracing::info!(target: "neutra_helper::protocol", "authentication boundary passed; entering command loop");
        }
        Some(ClientMsg::Hello { proto }) => {
            send(
                out,
                &HelperMsg::Error(format!(
                    "protocol mismatch: client={proto} helper={PROTO_VERSION}"
                )),
            )?;
            return Ok(false);
        }
        _ => {
            send(out, &HelperMsg::Error("expected Hello".into()))?;
            return Ok(false);
        }
    }

    Ok(true)
}
