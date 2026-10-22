use super::*;

pub(crate) struct HashingWriter<W> {
    pub(crate) inner: W,
    pub(crate) hasher: crc32fast::Hasher,
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(bytes)?;
        self.hasher.update(&bytes[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

pub(crate) fn open_private_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true).read(true);
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
    options.open(path)
}

pub(super) fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = temporary_path(path);
    let mut file = open_private_file(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    replace_file(&temporary, path)?;
    sync_parent(path)
}

/// Publish a validated staged sidecar without rewriting its bytes under the
/// store lock; fall back to a copy when hard links are unavailable.
pub(super) fn link_or_write_atomic(staged: &Path, destination: &Path) -> io::Result<()> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(cfg!(windows))
        .open(staged)?
        .sync_all()?;
    let temporary = temporary_path(destination);
    if std::fs::hard_link(staged, &temporary).is_ok() {
        if let Err(error) = replace_file(&temporary, destination) {
            let _ = std::fs::remove_file(&temporary);
            return Err(error);
        }
        return sync_parent(destination);
    }
    write_atomic(destination, &read_sidecar_bytes(staged)?)
}

/// Delete temporaries that a killed process left beside `destination`. Their
/// names carry the writer's process id, so only those whose process has gone
/// are removed. This reads the index's own folder, never the indexed volumes.
fn sweep_dead_temporaries(destination: &Path) {
    let (Some(dir), Some(name)) = (
        destination.parent(),
        destination.file_name().and_then(|n| n.to_str()),
    ) else {
        return;
    };
    let prefix = format!("{name}.new-");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let file = entry.file_name();
        let Some(rest) = file.to_str().and_then(|f| f.strip_prefix(&prefix)) else {
            continue;
        };
        let Some(pid) = rest.split('-').next().and_then(|p| p.parse::<u32>().ok()) else {
            continue;
        };
        if pid != std::process::id() && !process_alive(pid) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(target_os = "linux")]
fn process_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

/// Other platforms keep every temporary, since liveness cannot be told cheaply.
#[cfg(not(target_os = "linux"))]
fn process_alive(_pid: u32) -> bool {
    true
}

/// A unique temporary beside `path`; also clears ones a crashed writer left.
pub(crate) fn temporary_path(path: &Path) -> PathBuf {
    sweep_dead_temporaries(path);
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut value = path.as_os_str().to_os_string();
    value.push(format!(".new-{}-{nonce}", std::process::id()));
    value.into()
}

pub(super) fn remove_if_present(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => sync_parent(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(not(windows))]
pub(super) fn replace_file(temporary: &Path, destination: &Path) -> io::Result<()> {
    std::fs::rename(temporary, destination)
}

#[cfg(windows)]
pub(super) fn replace_file(temporary: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    #[link(name = "kernel32")]
    extern "system" {
        fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
    }
    let existing = temporary
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let replacement = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        MoveFileExW(
            existing.as_ptr(),
            replacement.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(unix)]
pub(super) fn sync_parent(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(not(unix))]
pub(super) fn sync_parent(_path: &Path) -> io::Result<()> {
    Ok(())
}

pub(crate) fn codec(error: impl std::fmt::Display) -> io::Error {
    invalid(format!("directory summary codec: {error}"))
}

pub(crate) fn invalid(error: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.into())
}

#[cfg(all(test, target_os = "linux"))]
mod sweep_tests {
    use super::*;

    #[test]
    fn removes_only_temporaries_of_dead_processes() {
        let dir = std::env::temp_dir().join(format!("ns-sweep-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("home.nsx.dirs");
        let dead = dir.join("home.nsx.dirs.new-4294967290-1");
        let mine = dir.join(format!("home.nsx.dirs.new-{}-1", std::process::id()));
        let other = dir.join("home.nsx.tree.new-4294967290-1");
        for f in [&dead, &mine, &other] {
            std::fs::write(f, "x").unwrap();
        }
        let _ = temporary_path(&dest);
        assert!(!dead.exists());
        assert!(mine.exists());
        assert!(other.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
