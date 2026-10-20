use std::io::{self, Read, Write};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::time::Duration;

mod focus;
#[cfg(any(target_os = "macos", test))]
mod runtime;
pub(super) use focus::{set_context, take_focus_request};

const MESSAGE: &[u8] = b"focus\n";

pub(super) enum Claim {
    Primary(Guard),
    FocusedExisting,
}

pub(super) struct Guard {
    path: PathBuf,
    inode: u64,
    device: u64,
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Ok(metadata) = std::fs::symlink_metadata(&self.path) {
            use std::os::unix::fs::MetadataExt;
            if metadata.ino() == self.inode && metadata.dev() == self.device {
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }
}

pub(super) fn claim_or_focus() -> io::Result<Claim> {
    let path = socket_path()?;
    match UnixListener::bind(&path) {
        Ok(listener) => {
            listener.set_nonblocking(true)?;
            let guard = guard_for(&path)?;
            let _ = std::thread::Builder::new()
                .name("spotlight-activation".into())
                .spawn(move || focus::accept(listener))?;
            Ok(Claim::Primary(guard))
        }
        Err(bind_error) if bind_error.kind() == io::ErrorKind::AddrInUse => {
            match focus_existing(&path) {
                Ok(()) => Ok(Claim::FocusedExisting),
                Err(_) => {
                    remove_stale_socket(&path)?;
                    let listener = UnixListener::bind(&path).map_err(|_| bind_error)?;
                    listener.set_nonblocking(true)?;
                    let guard = guard_for(&path)?;
                    let _ = std::thread::Builder::new()
                        .name("spotlight-activation".into())
                        .spawn(move || focus::accept(listener))?;
                    Ok(Claim::Primary(guard))
                }
            }
        }
        Err(error) => Err(error),
    }
}

fn guard_for(path: &PathBuf) -> io::Result<Guard> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(path)?;
    Ok(Guard {
        path: path.clone(),
        inode: metadata.ino(),
        device: metadata.dev(),
    })
}

fn socket_path() -> io::Result<PathBuf> {
    #[cfg(target_os = "macos")]
    let runtime = runtime::private_directory(std::env::temp_dir())?;
    #[cfg(not(target_os = "macos"))]
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute() && path.is_dir())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "XDG_RUNTIME_DIR is unavailable"))?;
    Ok(runtime.join("neutrasearch-quick-search.sock"))
}

fn focus_existing(path: &PathBuf) -> io::Result<()> {
    let mut stream = UnixStream::connect(path)?;
    stream.set_write_timeout(Some(Duration::from_millis(250)))?;
    stream.write_all(MESSAGE)
}

fn remove_stale_socket(path: &PathBuf) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_socket() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "instance path is not a socket",
        ));
    }
    use std::os::unix::fs::MetadataExt;
    let expected_uid = unsafe { libc::geteuid() };
    if metadata.uid() != expected_uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "instance socket belongs to another user",
        ));
    }
    match UnixStream::connect(path) {
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                "spotlight instance is active",
            ))
        }
        Err(error)
            if error.kind() == io::ErrorKind::ConnectionRefused
                || error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let current = std::fs::symlink_metadata(path)?;
    if current.ino() != metadata.ino() || current.dev() != metadata.dev() {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            "instance socket changed during stale-socket check",
        ));
    }
    std::fs::remove_file(path)
}
