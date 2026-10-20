use super::{file_actions, Action, Completion};

pub(super) fn execute(action: Action) -> Completion {
    execute_with(action, file_actions::execute)
}

fn execute_with(
    action: Action,
    mut execute: impl FnMut(Action) -> Result<Option<std::path::PathBuf>, String>,
) -> Completion {
    let restoring = matches!(action, Action::RestoreMany(_));
    let paths = match &action {
        Action::TrashMany(paths) | Action::RestoreMany(paths) => paths,
        _ => unreachable!("only trash/restore batches enter this worker"),
    };
    let mut tokens = Vec::new();
    let mut failures = 0;
    let mut first_error = None;
    for path in paths {
        let single = if restoring {
            Action::Restore(path.clone())
        } else {
            Action::Trash(path.clone())
        };
        match execute(single) {
            Ok(Some(token)) if !restoring => tokens.push(token),
            Ok(_) => {}
            Err(error) => {
                failures += 1;
                if first_error.is_none() {
                    first_error = Some(error);
                }
                if restoring {
                    tokens.push(path.clone());
                }
            }
        }
    }
    Completion::Batch {
        action,
        tokens,
        error: first_error.map(|error| format!("{failures} item(s) could not be changed: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn partial_restore_retains_only_failed_tokens() {
        let paths = vec![PathBuf::from("trash:///one"), PathBuf::from("trash:///two")];
        let completion = execute_with(Action::RestoreMany(paths.clone()), |action| {
            if matches!(action, Action::Restore(path) if path == paths[1]) {
                Err("destination already exists".into())
            } else {
                Ok(None)
            }
        });
        let Completion::Batch { tokens, error, .. } = completion else {
            panic!("batch completion");
        };
        assert_eq!(tokens, vec![paths[1].clone()]);
        assert_eq!(
            error.as_deref(),
            Some("1 item(s) could not be changed: destination already exists")
        );
    }

    #[test]
    fn partial_trash_preserves_successful_undo_tokens_and_reports_failure() {
        let paths = vec![PathBuf::from("/one"), PathBuf::from("/two")];
        let completion = execute_with(Action::TrashMany(paths), |action| {
            if matches!(action, Action::Trash(path) if path == std::path::Path::new("/one")) {
                Ok(Some(PathBuf::from("trash:///one")))
            } else {
                Err("permission denied".into())
            }
        });
        let Completion::Batch { tokens, error, .. } = completion else {
            panic!("batch completion");
        };
        assert_eq!(tokens, vec![PathBuf::from("trash:///one")]);
        assert!(error.unwrap().contains("permission denied"));
    }
}
