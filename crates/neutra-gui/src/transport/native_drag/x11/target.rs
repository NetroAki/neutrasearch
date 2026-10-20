use super::{atoms::Atoms, err};
use x11rb::{
    connection::Connection,
    protocol::xproto::{Atom, AtomEnum, ConnectionExt, Window},
    rust_connection::RustConnection,
    NONE,
};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Target {
    pub window: Window,
    pub proxy: Window,
    pub version: u32,
}

pub(super) fn find(conn: &RustConnection, root: Window, atoms: &Atoms) -> Result<Target, String> {
    let mut node = root;
    for _ in 0..128 {
        let child = conn
            .query_pointer(node)
            .map_err(err)?
            .reply()
            .map_err(err)?
            .child;
        if child == NONE {
            break;
        }
        node = child;
    }
    for _ in 0..128 {
        if let Some(version) = property(conn, node, atoms.aware)?.filter(|v| *v >= 3) {
            let proxy = property(conn, node, atoms.proxy)?.unwrap_or(node);
            if proxy != node && property(conn, proxy, atoms.proxy)? != Some(proxy) {
                return Ok(Target::default());
            }
            return Ok(Target {
                window: node,
                proxy,
                version: version.min(5),
            });
        }
        if node == root || node == NONE {
            break;
        }
        node = conn
            .query_tree(node)
            .map_err(err)?
            .reply()
            .map_err(err)?
            .parent;
    }
    Ok(Target::default())
}
fn property(conn: &RustConnection, window: Window, property: Atom) -> Result<Option<u32>, String> {
    let reply = conn
        .get_property(false, window, property, AtomEnum::ANY, 0, 1)
        .map_err(err)?
        .reply()
        .map_err(err)?;
    Ok(reply.value32().and_then(|mut v| v.next()))
}
pub(super) fn escape_keycode(conn: &RustConnection) -> Option<u8> {
    let setup = conn.setup();
    let map = conn
        .get_keyboard_mapping(setup.min_keycode, setup.max_keycode - setup.min_keycode + 1)
        .ok()?
        .reply()
        .ok()?;
    if map.keysyms_per_keycode == 0 {
        return None;
    }
    map.keysyms
        .chunks(map.keysyms_per_keycode as usize)
        .enumerate()
        .find_map(|(i, keys)| {
            keys.contains(&0xff1b)
                .then_some(setup.min_keycode + i as u8)
        })
}
