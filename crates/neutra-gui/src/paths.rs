use std::path::PathBuf;

pub(crate) fn env_flag(name: &str) -> bool {
    std::env::var_os(name).is_some()
}
pub(crate) fn configured_index() -> Option<PathBuf> {
    std::env::var_os("NEUTRASEARCH_INDEX").map(PathBuf::from)
}
pub(crate) fn legacy_cache_path() -> PathBuf {
    if let Some(path) = configured_index() {
        return path;
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(std::env::temp_dir)
            .join("Neutrasearch/index.bin")
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .map(|home| home.join("Library/Caches"))
            .unwrap_or_else(std::env::temp_dir)
            .join("Neutrasearch/index.bin")
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .filter(|path| path.is_absolute())
                    .map(|home| home.join(".cache"))
            })
            .unwrap_or_else(std::env::temp_dir)
            .join("neutrasearch/index.bin")
    }
}
pub(crate) fn compact_cache_path() -> PathBuf {
    neutra_core::paths::resolve_index_path(None)
}
