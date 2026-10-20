use super::*;

pub(super) fn spawn(
    jobs: Receiver<(Action, bool)>,
    done: Sender<Completion>,
    context: egui::Context,
) {
    std::thread::Builder::new()
        .name("file-operations".into())
        .spawn(move || {
            while let Ok((action, undo)) = jobs.recv() {
                let _ = done.send(execute(action, undo));
                context.request_repaint();
            }
        })
        .expect("file-operation worker");
}

fn execute(action: Action, undo: bool) -> Completion {
    match &action {
        Action::TrashMany(_) | Action::RestoreMany(_) => batch::execute(action),
        Action::OpenWith { path, desktop_id } if desktop_id.is_empty() => {
            Completion::Apps(path.clone(), file_actions::desktop_apps(path))
        }
        _ => {
            let result = file_actions::execute(action.clone());
            Completion::Action(action, undo, result)
        }
    }
}
