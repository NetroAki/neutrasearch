use super::run_with_env;
use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
#[path = "local_trash/mod.rs"]
mod local;

pub(super) fn trash(path: &Path) -> Result<PathBuf, String> {
    trash_with_env(path, None)
}
pub(super) fn restore(token: &Path) -> Result<PathBuf, String> {
    restore_with_env(token, None)
}

pub(super) fn trash_with_env(path: &Path, data_home: Option<&Path>) -> Result<PathBuf, String> {
    #[cfg(target_os = "linux")]
    {
        let before = items(data_home)?;
        if let Err(error) =
            run_with_env("gio", &["trash", "--", &path.to_string_lossy()], data_home)
        {
            return local::trash(path).map_err(|local| format!("{error}; local Trash: {local}"));
        }
        let matches: Vec<_> = items(data_home)?
            .into_iter()
            .filter(|(uri, original)| original == path && !before.iter().any(|(old, _)| old == uri))
            .map(|(uri, _)| uri)
            .collect();
        match matches.as_slice() {
            [uri] => Ok(uri.clone()),
            _ => Err("File was moved to Trash, but its undo location could not be identified. Restore it using your file manager.".into()),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (path, data_home);
        Err("Trash is currently supported on Linux only".into())
    }
}

pub(super) fn restore_with_env(token: &Path, data_home: Option<&Path>) -> Result<PathBuf, String> {
    #[cfg(target_os = "linux")]
    {
        if token.is_absolute() {
            return local::restore(token);
        }
        let original = items(data_home)?
            .into_iter()
            .find_map(|(uri, original)| (uri == token).then_some(original))
            .ok_or("Trash item no longer exists")?;
        // GIO resolves per-mount trash and relative metadata paths. Without
        // --force it refuses existing destinations, including dangling symlinks.
        run_with_env(
            "gio",
            &["trash", "--restore", "--", &token.to_string_lossy()],
            data_home,
        )?;
        Ok(original)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (token, data_home);
        Err("Restore is currently supported on Linux only".into())
    }
}

#[cfg(target_os = "linux")]
fn items(data_home: Option<&Path>) -> Result<Vec<(PathBuf, PathBuf)>, String> {
    let text = run_with_env("gio", &["trash", "--list"], data_home)?;
    Ok(text
        .lines()
        .filter_map(|line| {
            let (uri, original) = line.split_once('\t')?;
            uri.starts_with("trash:///")
                .then(|| (PathBuf::from(uri), PathBuf::from(original)))
        })
        .collect())
}
