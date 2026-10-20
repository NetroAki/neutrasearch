use super::*;

impl FileOperations {
    pub(crate) fn update(&mut self, ctx: &egui::Context) -> bool {
        let mut changed = false;
        let mut received = false;
        while let Ok(completion) = self.rx.try_recv() {
            self.pending = self.pending.saturating_sub(1);
            received = true;
            changed |= self.complete(completion);
        }
        if received {
            ctx.request_repaint();
        }
        if self.pending == 0 && self.undo_when_idle {
            self.undo_when_idle = false;
            self.undo();
        }
        changed
    }

    fn complete(&mut self, completion: Completion) -> bool {
        match completion {
            Completion::Apps(path, apps) => {
                self.open_with = Some((path, apps));
                false
            }
            Completion::Batch {
                action,
                tokens,
                error,
            } => {
                self.complete_batch(action, tokens, error);
                true
            }
            Completion::Action(action, undo, result) => self.complete_action(action, undo, result),
        }
    }

    fn complete_batch(&mut self, action: Action, tokens: Vec<PathBuf>, error: Option<String>) {
        if matches!(action, Action::RestoreMany(_)) {
            self.undo_pending = false;
            if tokens.is_empty() {
                self.undo.pop();
            } else if let Some(last) = self.undo.last_mut() {
                *last = Undo::TrashMany(tokens);
            }
        } else if !tokens.is_empty() {
            self.undo.push(Undo::TrashMany(tokens));
        }
        match error {
            Some(error) => self.message = Some((error, true)),
            None => self.completed_message(&action),
        }
    }

    fn complete_action(
        &mut self,
        action: Action,
        undo: bool,
        result: Result<Option<PathBuf>, String>,
    ) -> bool {
        if undo {
            self.undo_pending = false;
        }
        let token = match result {
            Ok(token) => token,
            Err(error) => {
                if let Action::Rename { from, .. } = action {
                    if !undo {
                        self.rename = Some(from);
                        self.rename_focus_pending = true;
                    }
                }
                self.message = Some((error, true));
                return false;
            }
        };
        let changed = matches!(
            action,
            Action::Trash(_)
                | Action::Restore(_)
                | Action::Rename { .. }
                | Action::Paste(_)
                | Action::Transfer { .. }
        );
        self.completed_message(&action);
        if undo {
            self.undo.pop();
        } else {
            self.remember_undo(action, token);
        }
        changed
    }

    fn remember_undo(&mut self, action: Action, token: Option<PathBuf>) {
        match action {
            Action::Trash(_) => {
                if let Some(token) = token {
                    self.undo.push(Undo::Trash(token));
                }
            }
            Action::Rename { from, to } => self.undo.push(Undo::Rename { from: to, to: from }),
            _ => {}
        }
    }

    fn completed_message(&mut self, action: &Action) {
        if self.pending == 0 {
            self.message = Some((format!("{} completed", action.label()), false));
        }
    }
}
