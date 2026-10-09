//! Background worker and UI state for explorer file operations.
use crate::transport::file_actions::{self, Action};
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    thread,
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
}
enum Completion {
    Action(Action, bool, Result<Option<PathBuf>, String>),
    Apps(PathBuf, Vec<DesktopApp>),
}
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
    pub(crate) fn new() -> Self {
        let (tx, jobs) = mpsc::channel::<(Action, bool)>();
        let (done, rx) = mpsc::channel();
        thread::Builder::new()
            .name("file-operations".into())
            .spawn(move || {
                while let Ok((a, undo)) = jobs.recv() {
                    if let Action::OpenWith { path, desktop_id } = &a {
                        if desktop_id.is_empty() {
                            let apps = file_actions::desktop_apps(path);
                            let _ = done.send(Completion::Apps(path.clone(), apps));
                            continue;
                        }
                    }
                    let r = file_actions::execute(a.clone());
                    let _ = done.send(Completion::Action(a, undo, r));
                }
            })
            .expect("file-operation worker");
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
        }
    }
    pub(crate) fn confirm_rename(&mut self) {
        if let Some(from) = self.rename.take() {
            let name = self.rename_name.trim();
            if name.is_empty() || name == "." || name == ".." || name.contains('/') {
                self.message = Some(("Enter a valid file name".into(), true));
                return;
            }
            let to = from
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .join(name);
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
            };
            if self.tx.send((action, true)).is_err() {
                self.message = Some(("Cannot queue undo".into(), true));
            } else {
                self.undo_pending = true;
                self.pending += 1;
            }
        }
    }
    pub(crate) fn update(&mut self, ctx: &egui::Context) -> bool {
        let mut changed = false;
        let mut received = false;
        while let Ok(c) = self.rx.try_recv() {
            self.pending = self.pending.saturating_sub(1);
            received = true;
            match c {
                Completion::Apps(p, a) => self.open_with = Some((p, a)),
                Completion::Action(a, undo, r) => {
                    if undo {
                        self.undo_pending = false;
                    }
                    match r {
                        Ok(token) => {
                            changed |= matches!(
                                a,
                                Action::Trash(_)
                                    | Action::Restore(_)
                                    | Action::Rename { .. }
                                    | Action::Paste(_)
                                    | Action::Transfer { .. }
                            );
                            self.message = Some((format!("{} completed", a.label()), false));
                            if undo {
                                self.undo.pop();
                            } else {
                                match a {
                                    Action::Trash(_) => {
                                        if let Some(token) = token {
                                            self.undo.push(Undo::Trash(token));
                                        }
                                    }
                                    Action::Rename { from, to } => {
                                        self.undo.push(Undo::Rename { from: to, to: from });
                                    }
                                    _ => {
                                        let _ = token;
                                    }
                                }
                            }
                        }
                        Err(e) => self.message = Some((e, true)),
                    }
                }
            }
        }
        if received {
            ctx.request_repaint()
        }
        if self.pending == 0 && self.undo_when_idle {
            self.undo_when_idle = false;
            self.undo();
        }
        changed
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
            Self::Restore(_) => "Restore",
            Self::Copy(_) => "Copy",
            Self::Cut(_) => "Cut",
            Self::Paste(_) => "Paste",
            Self::Transfer { .. } => "Transfer",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settle(ops: &mut FileOperations) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while ops.pending != 0 {
            ops.update(&egui::Context::default());
            assert!(
                std::time::Instant::now() < deadline,
                "file operation did not finish"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn rename_undo_keeps_failed_entries_and_never_becomes_redo() {
        let root = std::env::temp_dir().join(format!("neutra-undo-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let original = root.join("original");
        let renamed = root.join("renamed");
        let second = root.join("second");
        std::fs::write(&original, "preserve me").unwrap();
        let mut ops = FileOperations::new();
        ops.send(Action::Rename {
            from: original.clone(),
            to: renamed.clone(),
        });
        settle(&mut ops);
        ops.send(Action::Rename {
            from: renamed.clone(),
            to: second.clone(),
        });
        settle(&mut ops);
        ops.undo();
        settle(&mut ops);
        assert!(renamed.exists());
        assert!(!second.exists());
        std::fs::write(&original, "conflict").unwrap();
        ops.undo();
        settle(&mut ops);
        assert_eq!(ops.undo.len(), 1);
        assert_eq!(std::fs::read(&original).unwrap(), b"conflict");
        std::fs::remove_file(&original).unwrap();
        ops.undo();
        settle(&mut ops);
        assert_eq!(std::fs::read(&original).unwrap(), b"preserve me");
        assert!(ops.undo.is_empty());
        ops.undo();
        assert!(original.exists());
        assert!(!renamed.exists());
        ops.send(Action::Rename {
            from: original.clone(),
            to: renamed.clone(),
        });
        ops.undo();
        settle(&mut ops);
        assert!(
            original.exists(),
            "undo pressed during an operation must run after it completes"
        );
        assert!(!renamed.exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
