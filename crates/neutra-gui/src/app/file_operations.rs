//! Background worker and UI state for explorer file operations.
use crate::transport::file_actions::{self, Action};
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
};

#[derive(Clone, Debug)]
pub(crate) struct DesktopApp {
    pub(crate) id: String,
    pub(crate) name: String,
}
#[derive(Clone, Debug)]
pub(crate) enum Undo {
    Rename { from: PathBuf, to: PathBuf },
    Trash(PathBuf),
    TrashMany(Vec<PathBuf>),
}
enum Completion {
    Action(Action, bool, Result<Option<PathBuf>, String>),
    Apps(PathBuf, Vec<DesktopApp>),
    Batch {
        action: Action,
        tokens: Vec<PathBuf>,
        error: Option<String>,
    },
}
#[path = "file_operations/batch.rs"]
mod batch;
mod completion;
mod worker;
pub(crate) struct FileOperations {
    tx: Sender<(Action, bool)>,
    rx: Receiver<Completion>,
    pub(crate) message: Option<(String, bool)>,
    pub(crate) rename: Option<PathBuf>,
    pub(crate) rename_name: String,
    pub(crate) rename_focus_pending: bool,
    pub(crate) open_with: Option<(PathBuf, Vec<DesktopApp>)>,
    undo: Vec<Undo>,
    undo_pending: bool,
    pending: usize,
    undo_when_idle: bool,
}
impl FileOperations {
    pub(crate) fn new(context: egui::Context) -> Self {
        let (tx, jobs) = mpsc::channel::<(Action, bool)>();
        let (done, rx) = mpsc::channel();
        worker::spawn(jobs, done, context);
        Self {
            tx,
            rx,
            message: None,
            rename: None,
            rename_name: String::new(),
            rename_focus_pending: false,
            open_with: None,
            undo: Vec::new(),
            undo_pending: false,
            pending: 0,
            undo_when_idle: false,
        }
    }
    pub(crate) fn dispatch(&mut self, a: Action) {
        match a {
            Action::Rename { from, .. } => {
                self.rename_name = from
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                self.rename = Some(from);
                self.rename_focus_pending = true;
            }
            Action::OpenWith { path, .. } => self.send(Action::OpenWith {
                path,
                desktop_id: String::new(),
            }),
            a => self.send(a),
        }
    }
    fn send(&mut self, a: Action) {
        if let Err(e) = self.tx.send((a, false)) {
            self.message = Some((format!("Cannot queue file action: {e}"), true));
        } else {
            self.pending += 1;
            self.message = None;
        }
    }
    pub(crate) fn confirm_rename(&mut self) {
        if let Some(from) = self.rename.clone() {
            let name = self.rename_name.trim();
            if name.is_empty() || name == "." || name == ".." || name.contains('/') {
                self.message = Some(("Enter a valid file name".into(), true));
                return;
            }
            let to = from
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .join(name);
            self.rename = None;
            self.send(Action::Rename { from, to });
        }
    }
    pub(crate) fn choose_open_with(&mut self, id: String) {
        if let Some((path, _)) = self.open_with.take() {
            self.send(Action::OpenWith {
                path,
                desktop_id: id,
            });
        }
    }
    pub(crate) fn undo(&mut self) {
        if self.pending != 0 {
            self.undo_when_idle = true;
            return;
        }
        if self.undo_pending {
            return;
        }
        if let Some(u) = self.undo.last().cloned() {
            let action = match u {
                Undo::Rename { from, to } => Action::Rename { from, to },
                Undo::Trash(uri) => Action::Restore(uri),
                Undo::TrashMany(uris) => Action::RestoreMany(uris),
            };
            if self.tx.send((action, true)).is_err() {
                self.message = Some(("Cannot queue undo".into(), true));
            } else {
                self.undo_pending = true;
                self.pending += 1;
                self.message = None;
            }
        }
    }
}
impl Action {
    fn label(&self) -> &'static str {
        match self {
            Self::Open(_) => "Open",
            Self::OpenWith { .. } => "Open with",
            Self::Reveal(_) => "Reveal",
            Self::Rename { .. } => "Rename",
            Self::Trash(_) => "Trash",
            Self::TrashMany(_) => "Trash",
            Self::RestoreMany(_) => "Restore",
            Self::Restore(_) => "Restore",
            Self::Copy(_) => "Copy",
            Self::Cut(_) => "Cut",
            Self::Paste(_) => "Paste",
            Self::Transfer { .. } => "Transfer",
        }
    }
}

#[cfg(test)]
#[path = "file_operations/tests.rs"]
mod tests;
