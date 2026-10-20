use crate::{normalize_selected_root, same_root, GuiSettings};
use std::io::Write;
use std::path::PathBuf;

pub(crate) fn gui_settings_path() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(std::env::temp_dir)
            .join("Neutrasearch/gui-settings.json")
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .map(|home| home.join("Library/Application Support"))
            .unwrap_or_else(std::env::temp_dir)
            .join("Neutrasearch/gui-settings.json")
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .filter(|path| path.is_absolute())
                    .map(|home| home.join(".config"))
            })
            .unwrap_or_else(std::env::temp_dir)
            .join("neutrasearch/gui-settings.json")
    }
}

pub(crate) fn load_gui_settings(path: &std::path::Path) -> Option<GuiSettings> {
    let bytes = std::fs::read(path).ok()?;
    let mut settings: GuiSettings = serde_json::from_slice(&bytes).ok()?;
    settings.roots = settings
        .roots
        .into_iter()
        .map(normalize_selected_root)
        .filter(|root| root.is_absolute())
        .collect();
    settings.roots.sort();
    settings
        .roots
        .dedup_by(|left, right| same_root(left, right));
    Some(settings)
}

pub(crate) fn save_gui_settings(
    path: &std::path::Path,
    settings: &GuiSettings,
) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "settings path has no parent".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create settings directory: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("cannot protect settings directory: {error}"))?;
    }
    let temporary = path.with_extension("json.new");
    let bytes = serde_json::to_vec_pretty(settings)
        .map_err(|error| format!("cannot encode settings: {error}"))?;
    let _ = std::fs::remove_file(&temporary);
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|error| format!("cannot create settings: {error}"))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("cannot write settings: {error}"))?;
    drop(file);
    publish_settings(&temporary, path).map_err(|error| format!("cannot publish settings: {error}"))
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn publish_settings(
    temporary: &std::path::Path,
    destination: &std::path::Path,
) -> std::io::Result<()> {
    std::fs::rename(temporary, destination)
}

#[cfg(target_os = "windows")]
pub(crate) fn publish_settings(
    temporary: &std::path::Path,
    destination: &std::path::Path,
) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    extern "system" {
        fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
    }
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    let existing = temporary
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let replacement = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        MoveFileExW(
            existing.as_ptr(),
            replacement.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}
