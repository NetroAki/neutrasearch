use super::{selection, session::Session, target, DragAction, DragEvent};
use std::{sync::mpsc::Sender, time::Instant};
use x11rb::{
    protocol::{xproto::MotionNotifyEvent, Event},
    NONE,
};

impl Session {
    pub fn handle(
        &mut self,
        event: Event,
        events: &Sender<DragEvent>,
    ) -> Result<Option<DragEvent>, String> {
        match event {
            Event::KeyPress(key) if Some(key.detail) == self.escape => {
                return Ok(Some(DragEvent::Cancelled))
            }
            Event::SelectionClear(clear) if clear.selection == self.atoms.selection => {
                return Ok(Some(DragEvent::Cancelled))
            }
            Event::ButtonRelease(release) if release.detail == 1 && !self.dropped => {
                self.released = Some((Instant::now(), release.time));
                if self.target.window == NONE {
                    return Ok(Some(DragEvent::Cancelled));
                }
                if !self.awaiting_status {
                    return self.drop_if_accepted();
                }
            }
            Event::MotionNotify(motion) if self.released.is_none() => self.motion(motion)?,
            Event::ClientMessage(message)
                if message.format == 32 && message.data.as_data32()[0] == self.target.window =>
            {
                let data = message.data.as_data32();
                if message.type_ == self.atoms.status && !self.dropped {
                    self.awaiting_status = false;
                    self.accepted = data[1] & 1 != 0 && data[4] == self.atoms.action_copy;
                    if self.accepted {
                        let _ = events.send(DragEvent::Offered {
                            actions: vec![DragAction::Copy],
                        });
                    }
                    if self.released.is_some() {
                        return self.drop_if_accepted();
                    }
                } else if message.type_ == self.atoms.finished && self.dropped {
                    return Ok(Some(
                        if self.target.version < 5
                            || data[1] & 1 != 0 && data[2] == self.atoms.action_copy
                        {
                            DragEvent::Finished {
                                action: DragAction::Copy,
                            }
                        } else {
                            DragEvent::Cancelled
                        },
                    ));
                }
            }
            Event::SelectionRequest(request)
                if request.selection == self.atoms.selection && request.owner == self.source =>
            {
                selection::serve(&self.conn, &self.atoms, request, &self.bytes)?;
            }
            _ => {}
        }
        Ok(None)
    }
    fn drop_if_accepted(&mut self) -> Result<Option<DragEvent>, String> {
        if !self.accepted {
            return Ok(Some(DragEvent::Cancelled));
        }
        let timestamp = self.released.map(|(_, time)| time).unwrap_or(0);
        self.send(self.atoms.drop, [self.source, 0, timestamp, 0, 0])?;
        self.dropped = true;
        Ok(None)
    }
    fn motion(&mut self, motion: MotionNotifyEvent) -> Result<(), String> {
        let next = target::find(&self.conn, self.root, &self.atoms)?;
        if next != self.target {
            if self.target.window != NONE {
                self.send(self.atoms.leave, [self.source, 0, 0, 0, 0])?;
            }
            self.target = next;
            self.accepted = false;
            self.awaiting_status = false;
            self.last_position = None;
            if self.target.window != NONE {
                self.send(
                    self.atoms.enter,
                    [
                        self.source,
                        self.target.version << 24,
                        self.atoms.uri_list,
                        0,
                        0,
                    ],
                )?;
            }
        }
        let point = (motion.root_x, motion.root_y);
        if self.target.window != NONE && self.last_position != Some(point) {
            self.last_position = Some(point);
            let packed = ((motion.root_x as u16 as u32) << 16) | motion.root_y as u16 as u32;
            self.send(
                self.atoms.position,
                [self.source, 0, packed, motion.time, self.atoms.action_copy],
            )?;
            self.awaiting_status = true;
        }
        Ok(())
    }
}
