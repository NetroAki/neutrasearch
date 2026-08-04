//! Shared resolution for the durable machine index.
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Resolve an explicit override, configured environment path, most recently
/// built index, or the platform default—in that order.
pub fn resolve_index_path(explicit: Option<PathBuf>) -> PathBuf {
    resolve_index_path_from(
        explicit,
        configured_index_path(),
        last_index_path(),
        default_index_path(),
    )
}

fn resolve_index_path_from(
    explicit: Option<PathBuf>,
    configured: Option<PathBuf>,
    remembered: Option<PathBuf>,
    default: PathBuf,
) -> PathBuf {
    explicit.or(configured).or(remembered).unwrap_or(default)
}

pub fn configured_index_path() -> Option<PathBuf> {
    std::env::var_os("NEUTRASEARCH_INDEX")
        .or_else(|| std::env::var_os("NEUTRA_INDEX"))
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}

pub fn last_index_path() -> Option<PathBuf> {
    read_pointer(&index_pointer_path()).ok()
}

/// Persist the location selected by a successful full-machine build.
pub fn remember_index_path(path: &Path) -> io::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let absolute = std::fs::canonicalize(&absolute).unwrap_or(absolute);
    if last_index_path().as_ref() == Some(&absolute) {
        return Ok(absolute);
    }
    write_pointer(&index_pointer_path(), &absolute)?;
    Ok(absolute)
}

pub fn default_index_path() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(std::env::temp_dir)
            .join("Neutrasearch/index.nsx")
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .map(|home| home.join("Library/Application Support"))
            .unwrap_or_else(std::env::temp_dir)
            .join("Neutrasearch/index.nsx")
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .filter(|path| path.is_absolute())
                    .map(|home| home.join(".local/share"))
            })
            .unwrap_or_else(std::env::temp_dir)
            .join("neutrasearch/index.nsx")
    }
}

fn index_pointer_path() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(std::env::temp_dir)
            .join("Neutrasearch/last-index.bin")
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .map(|home| home.join("Library/Application Support"))
            .unwrap_or_else(std::env::temp_dir)
            .join("Neutrasearch/last-index.bin")
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
            .join("neutrasearch/last-index.bin")
    }
}

fn read_pointer(path: &Path) -> io::Result<PathBuf> {
    let bytes = std::fs::read(path)?;
    let index: PathBuf = bincode::deserialize(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if !index.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "remembered index path is not absolute",
        ));
    }
    Ok(index)
}

fn write_pointer(path: &Path, index: &Path) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "pointer has no parent"))?;
    std::fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = path.with_extension(format!("new-{}-{nonce}", std::process::id()));
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(&bincode::serialize(index).map_err(io::Error::other)?)?;
    file.sync_all()?;
    drop(file);
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    std::fs::rename(&temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_configured_and_remembered_locations_have_stable_precedence() {
        let default = PathBuf::from("/default/index.nsx");
        let remembered = PathBuf::from("/remembered/index.nsx");
        let configured = PathBuf::from("/configured/index.nsx");
        let explicit = PathBuf::from("/explicit/index.nsx");
        assert_eq!(
            resolve_index_path_from(
                Some(explicit.clone()),
                Some(configured.clone()),
                Some(remembered.clone()),
                default.clone(),
            ),
            explicit
        );
        assert_eq!(
            resolve_index_path_from(
                None,
                Some(configured.clone()),
                Some(remembered.clone()),
                default.clone(),
            ),
            configured
        );
        assert_eq!(
            resolve_index_path_from(None, None, Some(remembered.clone()), default.clone()),
            remembered
        );
        assert_eq!(
            resolve_index_path_from(None, None, None, default.clone()),
            default
        );
    }

    #[test]
    fn pointer_roundtrip_requires_an_absolute_path() {
        let root =
            std::env::temp_dir().join(format!("neutra-index-pointer-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let pointer = root.join("last-index.bin");
        let index = root.join("index.nsx");
        write_pointer(&pointer, &index).unwrap();
        assert_eq!(read_pointer(&pointer).unwrap(), index);
        std::fs::remove_file(pointer).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
}
