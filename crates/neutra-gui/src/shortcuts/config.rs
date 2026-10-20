use std::path::PathBuf;

pub(crate) fn config_path() -> PathBuf {
    crate::gui_settings_path().with_file_name("shortcuts.json")
}

pub(crate) fn load() -> Result<Vec<String>, String> {
    let bytes = match std::fs::read(config_path()) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(vec![super::DEFAULT.into()])
        }
        Err(error) => return Err(format!("Cannot read keyboard shortcuts: {error}")),
    };
    let keys: Vec<String> = serde_json::from_slice(&bytes)
        .map_err(|e| format!("Cannot read keyboard shortcuts: {e}"))?;
    super::normalize(&keys)
}

pub(super) fn save(keys: &[String]) -> Result<(), String> {
    save_at(&config_path(), keys)
}

pub(super) fn snapshot() -> Result<Option<Vec<u8>>, String> {
    match std::fs::read(config_path()) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("Cannot read keyboard shortcuts: {error}")),
    }
}

pub(super) fn restore(snapshot: Option<Vec<u8>>) -> Result<(), String> {
    match snapshot {
        Some(bytes) => write_at(&config_path(), &bytes),
        None => std::fs::remove_file(config_path()).map_err(|e| e.to_string()),
    }
}

fn save_at(path: &std::path::Path, keys: &[String]) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(keys).map_err(|e| e.to_string())?;
    write_at(path, &bytes)
}

fn write_at(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let parent = path
        .parent()
        .ok_or("Shortcut settings have no parent directory")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let temporary = path.with_extension("json.new");
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|e| format!("Cannot save keyboard shortcuts: {e}"))?;
    let result = (|| {
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|e| e.to_string())?;
        drop(file);
        crate::settings::publish_settings(&temporary, path).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
pub(super) fn apply_native(keys: &[String]) -> Result<(), String> {
    let previous = snapshot()?;
    match std::fs::remove_file(config_path().with_extension("status.json")) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("Cannot refresh shortcut status: {error}")),
    }
    save(keys)?;
    let result = super::native::install_autostart().and_then(|()| wait_native(keys));
    if let Err(error) = result {
        restore(previous)
            .map_err(|rollback| format!("{error}; cannot restore saved shortcuts: {rollback}"))?;
        return Err(error);
    }
    Ok(())
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn wait_native(keys: &[String]) -> Result<(), String> {
    #[derive(serde::Deserialize)]
    struct Status {
        keys: Vec<String>,
        error: Option<String>,
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if let Ok(bytes) = std::fs::read(config_path().with_extension("status.json")) {
            if let Ok(status) = serde_json::from_slice::<Status>(&bytes) {
                if status.keys == keys {
                    return status.error.map_or(Ok(()), Err);
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Err("The global shortcut service did not confirm the change".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saved_shortcuts_replace_atomically_and_keep_existing_on_failure() {
        let directory =
            std::env::temp_dir().join(format!("neutrasearch-shortcut-save-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("shortcuts.json");
        save_at(&path, &[super::super::DEFAULT.into()]).unwrap();
        save_at(&path, &["shift+alt+KeyF".into()]).unwrap();
        let before = std::fs::read(&path).unwrap();
        std::fs::write(path.with_extension("json.new"), "another writer").unwrap();
        assert!(save_at(&path, &[]).is_err());
        assert_eq!(std::fs::read(path).unwrap(), before);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
