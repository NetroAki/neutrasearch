use super::{
    atoms::Atoms,
    err, selection,
    target::{self, Target},
};
use std::time::Instant;
use x11rb::{
    connection::Connection,
    protocol::xproto::{
        AtomEnum, ConnectionExt, EventMask, GrabMode, GrabStatus, PropMode, Window, WindowClass,
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
    CURRENT_TIME, NONE,
};

pub(super) struct Session {
    pub conn: RustConnection,
    pub root: Window,
    pub source: Window,
    pub atoms: Atoms,
    pub bytes: Vec<u8>,
    pub escape: Option<u8>,
    pub started: Instant,
    pub target: Target,
    pub accepted: bool,
    pub awaiting_status: bool,
    pub dropped: bool,
    pub released: Option<(Instant, u32)>,
    pub last_position: Option<(i16, i16)>,
}
impl Session {
    pub fn new(window: Window, bytes: Vec<u8>) -> Result<Self, String> {
        let (conn, screen_num) = x11rb::connect(None).map_err(err)?;
        let root = conn.setup().roots[screen_num].root;
        let atoms = Atoms::new(&conn)?;
        let source = conn.generate_id().map_err(err)?;
        conn.create_window(
            0,
            source,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            0,
            &Default::default(),
        )
        .map_err(err)?
        .check()
        .map_err(err)?;
        let session = Self {
            escape: target::escape_keycode(&conn),
            conn,
            root,
            source,
            atoms,
            bytes,
            started: Instant::now(),
            target: Target::default(),
            accepted: false,
            awaiting_status: false,
            dropped: false,
            released: None,
            last_position: None,
        };
        session.prepare(window)?;
        Ok(session)
    }
    fn prepare(&self, window: Window) -> Result<(), String> {
        self.conn
            .change_property32(
                PropMode::REPLACE,
                self.source,
                self.atoms.aware,
                AtomEnum::ATOM,
                &[5],
            )
            .map_err(err)?
            .check()
            .map_err(err)?;
        self.conn
            .change_property32(
                PropMode::REPLACE,
                self.source,
                self.atoms.action_list,
                AtomEnum::ATOM,
                &[self.atoms.action_copy],
            )
            .map_err(err)?
            .check()
            .map_err(err)?;
        self.conn
            .set_selection_owner(self.source, self.atoms.selection, CURRENT_TIME)
            .map_err(err)?
            .check()
            .map_err(err)?;
        if self
            .conn
            .get_selection_owner(self.atoms.selection)
            .map_err(err)?
            .reply()
            .map_err(err)?
            .owner
            != self.source
        {
            return Err("Cannot own file drag selection".into());
        }
        let mask = EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION;
        let pointer = self
            .conn
            .grab_pointer(
                false,
                window,
                mask,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
                NONE,
                NONE,
                CURRENT_TIME,
            )
            .map_err(err)?
            .reply()
            .map_err(err)?;
        if pointer.status != GrabStatus::SUCCESS {
            return Err(format!(
                "Cannot start file drag: pointer grab {:?}",
                pointer.status
            ));
        }
        let keyboard = self
            .conn
            .grab_keyboard(
                false,
                window,
                CURRENT_TIME,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
            )
            .map_err(err)?
            .reply()
            .map_err(err)?;
        if keyboard.status != GrabStatus::SUCCESS {
            return Err("Cannot acquire keyboard for drag cancellation".into());
        }
        self.conn.flush().map_err(err)
    }
    pub fn send(&self, kind: u32, data: [u32; 5]) -> Result<(), String> {
        selection::send(
            &self.conn,
            self.target.proxy,
            self.target.window,
            kind,
            data,
        )
    }
    pub fn release_timed_out(&self) -> bool {
        self.released
            .is_some_and(|(time, _)| !self.dropped && time.elapsed().as_millis() > 250)
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        if self.target.window != NONE && !self.dropped {
            let _ = self.send(self.atoms.leave, [self.source, 0, 0, 0, 0]);
        }
        let _ = self.conn.ungrab_keyboard(CURRENT_TIME);
        let _ = self.conn.ungrab_pointer(CURRENT_TIME);
        // A replacement drag may already own the selection when we cancel.
        if self
            .conn
            .get_selection_owner(self.atoms.selection)
            .ok()
            .and_then(|c| c.reply().ok())
            .is_some_and(|reply| reply.owner == self.source)
        {
            let _ = self
                .conn
                .set_selection_owner(NONE, self.atoms.selection, CURRENT_TIME);
        }
        let _ = self.conn.destroy_window(self.source);
        let _ = self.conn.flush();
    }
}
