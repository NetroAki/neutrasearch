use super::{atoms::Atoms, err};
use x11rb::{
    connection::{Connection, RequestConnection},
    protocol::xproto::{
        self, Atom, AtomEnum, ClientMessageEvent, ConnectionExt, EventMask, PropMode, Window,
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
    NONE,
};

pub(super) fn send(
    conn: &RustConnection,
    dest: Window,
    target: Window,
    kind: Atom,
    data: [u32; 5],
) -> Result<(), String> {
    conn.send_event(
        false,
        dest,
        EventMask::NO_EVENT,
        ClientMessageEvent::new(32, target, kind, data),
    )
    .map_err(err)?
    .check()
    .map_err(err)?;
    conn.flush().map_err(err)
}
pub(super) fn serve(
    conn: &RustConnection,
    atoms: &Atoms,
    req: xproto::SelectionRequestEvent,
    bytes: &[u8],
) -> Result<(), String> {
    let property = if req.property == NONE {
        req.target
    } else {
        req.property
    };
    let successful = if req.target == atoms.targets {
        conn.change_property32(
            PropMode::REPLACE,
            req.requestor,
            property,
            AtomEnum::ATOM,
            &[atoms.targets, atoms.uri_list],
        )
        .map_err(err)?
        .check()
        .map_err(err)?;
        true
    } else if req.target == atoms.uri_list && bytes.len() + 64 <= conn.maximum_request_bytes() {
        conn.change_property8(
            PropMode::REPLACE,
            req.requestor,
            property,
            atoms.uri_list,
            bytes,
        )
        .map_err(err)?
        .check()
        .map_err(err)?;
        true
    } else {
        false
    };
    let notify = xproto::SelectionNotifyEvent {
        response_type: xproto::SELECTION_NOTIFY_EVENT,
        sequence: 0,
        time: req.time,
        requestor: req.requestor,
        selection: req.selection,
        target: req.target,
        property: if successful { property } else { NONE },
    };
    conn.send_event(false, req.requestor, EventMask::NO_EVENT, notify)
        .map_err(err)?
        .check()
        .map_err(err)?;
    conn.flush().map_err(err)
}
