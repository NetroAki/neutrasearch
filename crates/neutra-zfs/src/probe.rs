#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZfsLaneProbe {
    pub zfs_cli_version: Option<String>,
    /// Discoverability does not establish compatibility with the native reader.
    pub libzpool_soname: Option<&'static str>,
}

pub fn probe() -> ZfsLaneProbe {
    let zfs_cli_version = std::process::Command::new("zfs")
        .arg("version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .unwrap_or_default()
                .trim()
                .to_owned()
        })
        .filter(|line| !line.is_empty());
    let libzpool_soname = [
        "libzpool.so.6",
        "libzpool.so.5",
        "libzpool.so.4",
        "libzpool.so.2",
    ]
    .into_iter()
    .find(|soname| library_available(soname));
    ZfsLaneProbe {
        zfs_cli_version,
        libzpool_soname,
    }
}

pub fn reader_version() -> anyhow::Result<String> {
    #[cfg(target_os = "linux")]
    {
        let output = crate::programs::command(&crate::programs::reader()?)
            .arg("--version")
            .output()?;
        anyhow::ensure!(
            output.status.success(),
            "Native ZFS reader failed its version probe"
        );
        Ok(String::from_utf8(output.stdout)?.trim().to_owned())
    }
    #[cfg(not(target_os = "linux"))]
    anyhow::bail!("The native ZFS reader requires Linux/OpenZFS")
}

#[cfg(unix)]
fn library_available(soname: &str) -> bool {
    let Ok(name) = std::ffi::CString::new(soname) else {
        return false;
    };
    // The system loader knows multiarch paths and configured library directories.
    unsafe {
        let handle = libc::dlopen(name.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL);
        if handle.is_null() {
            return false;
        }
        libc::dlclose(handle);
        true
    }
}

#[cfg(not(unix))]
fn library_available(_soname: &str) -> bool {
    false
}
