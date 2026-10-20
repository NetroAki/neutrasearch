#[cfg(not(unix))]
use crate::startup_access::validate_elevated_helper;
use std::path::PathBuf;

pub(crate) fn select_helper(
    configured: Option<PathBuf>,
    current_exe: Option<PathBuf>,
    elevated: bool,
) -> Result<PathBuf, String> {
    if elevated && configured.is_some() {
        return Err(
            "refusing to elevate a helper selected through NEUTRASEARCH_HELPER; install a trusted system helper"
                .into(),
        );
    }
    let sibling = current_exe.map(|path| {
        path.with_file_name(if cfg!(windows) {
            "neutrasearch-helper.exe"
        } else {
            "neutrasearch-helper"
        })
    });
    if elevated {
        #[cfg(unix)]
        let candidates = [
            PathBuf::from("/usr/local/lib/neutrasearch/neutrasearch-helper"),
            PathBuf::from("/usr/lib/neutrasearch/neutrasearch-helper"),
            PathBuf::from("/usr/local/bin/neutrasearch-helper"),
        ];
        #[cfg(not(unix))]
        let candidates = sibling.into_iter().collect::<Vec<_>>();
        for helper in candidates {
            if let Ok(helper) = validate_elevated_helper(&helper) {
                return Ok(helper);
            }
        }
        return Err(
            "no trusted administrator helper is installed; reinstall Neutrasearch system-wide"
                .into(),
        );
    }
    Ok(configured
        .or(sibling)
        .unwrap_or_else(|| PathBuf::from("neutrasearch-helper")))
}

#[cfg(unix)]
pub(crate) fn validate_elevated_helper(path: &std::path::Path) -> Result<PathBuf, String> {
    use std::os::unix::fs::MetadataExt;
    let path = std::fs::canonicalize(path).map_err(|error| {
        format!(
            "cannot resolve installed helper {}: {error}",
            path.display()
        )
    })?;
    let allowed = [
        std::path::Path::new("/usr/local/lib/neutrasearch"),
        std::path::Path::new("/usr/lib/neutrasearch"),
        std::path::Path::new("/usr/local/bin"),
    ];
    if !allowed.iter().any(|directory| path.starts_with(directory)) {
        return Err(format!(
            "refusing helper outside trusted system locations: {}",
            path.display()
        ));
    }
    let metadata = std::fs::metadata(&path).map_err(|error| {
        format!(
            "cannot inspect installed helper {}: {error}",
            path.display()
        )
    })?;
    if !metadata.is_file() || metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
        return Err(format!(
            "refusing to elevate untrusted helper {}; it must be a root-owned regular file not writable by group/others",
            path.display()
        ));
    }
    let mut ancestor = path.parent();
    while let Some(directory) = ancestor {
        let metadata = std::fs::metadata(directory).map_err(|error| {
            format!(
                "cannot inspect helper directory {}: {error}",
                directory.display()
            )
        })?;
        if !metadata.is_dir() || metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
            return Err(format!(
                "refusing helper beneath untrusted directory {}",
                directory.display()
            ));
        }
        ancestor = directory.parent();
    }
    Ok(path)
}
