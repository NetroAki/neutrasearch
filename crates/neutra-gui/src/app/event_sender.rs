use super::Event;
use std::sync::mpsc::{SendError, Sender};

#[derive(Clone)]
pub(crate) struct EventSender {
    sender: Sender<Event>,
    context: egui::Context,
}

impl EventSender {
    pub(crate) fn new(sender: Sender<Event>, context: egui::Context) -> Self {
        Self { sender, context }
    }

    pub(crate) fn send(&self, event: Event) -> Result<(), SendError<()>> {
        self.sender.send(event).map_err(|_| SendError(()))?;
        self.context.request_repaint();
        Ok(())
    }
}
