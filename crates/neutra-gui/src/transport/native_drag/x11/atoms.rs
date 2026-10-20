use super::err;
use x11rb::{
    protocol::xproto::{Atom, ConnectionExt},
    rust_connection::RustConnection,
};

pub(super) struct Atoms {
    pub selection: Atom,
    pub targets: Atom,
    pub uri_list: Atom,
    pub aware: Atom,
    pub proxy: Atom,
    pub enter: Atom,
    pub position: Atom,
    pub status: Atom,
    pub drop: Atom,
    pub finished: Atom,
    pub leave: Atom,
    pub action_copy: Atom,
    pub action_list: Atom,
}
impl Atoms {
    pub fn new(conn: &RustConnection) -> Result<Self, String> {
        let atom = |name: &[u8]| {
            conn.intern_atom(false, name)
                .map_err(err)?
                .reply()
                .map_err(err)
                .map(|r| r.atom)
        };
        Ok(Self {
            selection: atom(b"XdndSelection")?,
            targets: atom(b"TARGETS")?,
            uri_list: atom(b"text/uri-list")?,
            aware: atom(b"XdndAware")?,
            proxy: atom(b"XdndProxy")?,
            enter: atom(b"XdndEnter")?,
            position: atom(b"XdndPosition")?,
            status: atom(b"XdndStatus")?,
            drop: atom(b"XdndDrop")?,
            finished: atom(b"XdndFinished")?,
            leave: atom(b"XdndLeave")?,
            action_copy: atom(b"XdndActionCopy")?,
            action_list: atom(b"XdndActionList")?,
        })
    }
}
