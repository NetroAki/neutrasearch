use super::{file_actions, FileAction};

pub(crate) fn launch_file_action(action: FileAction) -> std::io::Result<()> {
    file_actions::execute(action)
        .map(|_| ())
        .map_err(std::io::Error::other)
}
