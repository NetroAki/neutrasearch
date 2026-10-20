//! Copy-only native file drags. The destination owns the actual copy;
//! protocol completion never authorizes deleting a source.

use std::path::{Path, PathBuf};

/// Result of trying to start a native drag. A platform implementation must
/// return its live source handle; unsupported backends stay visibly unsupported.
pub(crate) enum BeginError {
    Unsupported(Unsupported),
    Failed(String),
}

/// The operation negotiated by the receiving application.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DragAction {
    Copy,
}

/// Terminal and progress notifications emitted by a native platform driver.
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code, unused_imports))]
pub(crate) enum DragEvent {
    Started,
    /// A destination accepted a URI-list offer and advertised these actions.
    Offered {
        actions: Vec<DragAction>,
    },
    /// A destination confirmed that it accepted the drop.
    Finished {
        action: DragAction,
    },
    Cancelled,
    Failed(String),
}

/// Explicit outcome for a platform on which no native source is installed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Unsupported {
    pub platform: &'static str,
    pub reason: &'static str,
}

/// Native source lifecycle. Platform drivers own protocol state; callers poll
/// events from the GUI update loop and may cancel on Escape/window shutdown.
/// This API never removes or otherwise mutates source files.
pub(crate) trait NativeDragSource {
    fn poll(&mut self) -> Vec<DragEvent>;
    fn cancel(&mut self);
}

#[cfg(target_os = "linux")]
mod x11;

pub(crate) const MAX_OFFER_BYTES: usize = 1024 * 1024;

pub(crate) fn begin(
    window: &std::sync::Arc<winit::window::Window>,
    paths: &[PathBuf],
) -> Result<Box<dyn NativeDragSource>, BeginError> {
    let bytes = uri_list(paths).map_err(BeginError::Failed)?;
    if bytes.len() > MAX_OFFER_BYTES {
        return Err(BeginError::Failed(
            "Select fewer files: drag offers are limited to one MiB".into(),
        ));
    }
    #[cfg(target_os = "linux")]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let handle = window
            .window_handle()
            .map_err(|e| BeginError::Failed(e.to_string()))?;
        match handle.as_raw() {
            RawWindowHandle::Xlib(handle) => {
                use winit::platform::x11::WindowExtX11;
                window
                    .release_pointer_for_file_drag()
                    .map_err(BeginError::Failed)?;
                x11::begin(handle.window as u32, bytes)
            }
            RawWindowHandle::Xcb(handle) => {
                use winit::platform::x11::WindowExtX11;
                window
                    .release_pointer_for_file_drag()
                    .map_err(BeginError::Failed)?;
                x11::begin(handle.window.get(), bytes)
            }
            RawWindowHandle::Wayland(_) => {
                use winit::platform::wayland::WindowExtWayland;
                let status = window.drag_uri_list(bytes).map_err(BeginError::Failed)?;
                Ok(Box::new(WaylandSource {
                    window: window.clone(),
                    status,
                    reported: 0,
                }))
            }
            _ => Err(BeginError::Unsupported(unsupported("other"))),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (window, bytes);
        Err(BeginError::Unsupported(unsupported("other")))
    }
}

#[cfg(target_os = "linux")]
struct WaylandSource {
    window: std::sync::Arc<winit::window::Window>,
    status: std::sync::Arc<std::sync::atomic::AtomicU8>,
    reported: u8,
}
#[cfg(target_os = "linux")]
impl NativeDragSource for WaylandSource {
    fn poll(&mut self) -> Vec<DragEvent> {
        let status = self.status.load(std::sync::atomic::Ordering::Acquire);
        if status == self.reported {
            return Vec::new();
        }
        self.reported = status;
        match status {
            1 => vec![DragEvent::Started],
            2 => vec![DragEvent::Finished {
                action: DragAction::Copy,
            }],
            3 => vec![DragEvent::Cancelled],
            4 => vec![DragEvent::Failed(
                "The compositor could not complete the file drag".into(),
            )],
            _ => Vec::new(),
        }
    }
    fn cancel(&mut self) {
        if self.status.load(std::sync::atomic::Ordering::Acquire) < 2 {
            self.status.store(3, std::sync::atomic::Ordering::Release);
            self.window.request_redraw();
        }
    }
}
#[cfg(target_os = "linux")]
impl Drop for WaylandSource {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Build the RFC 8089 URI-list representation used by native file managers.
/// Non-UTF-8 Unix paths are encoded from their original bytes.
pub(crate) fn uri_list(paths: &[PathBuf]) -> Result<Vec<u8>, String> {
    if paths.is_empty() {
        return Err("cannot start a native drag with no paths".into());
    }
    let mut output = Vec::new();
    for path in paths {
        if !path.is_absolute() {
            return Err(format!(
                "native drag path is not absolute: {}",
                path.display()
            ));
        }
        output.extend_from_slice(path_to_file_uri(path)?.as_bytes());
        output.extend_from_slice(b"\r\n");
    }
    Ok(output)
}

fn path_to_file_uri(path: &Path) -> Result<String, String> {
    #[cfg(unix)]
    let bytes: &[u8] = {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes()
    };
    #[cfg(not(unix))]
    let owned = path
        .to_str()
        .ok_or_else(|| "path cannot be represented as a file URI".to_owned())?
        .as_bytes()
        .to_vec();
    #[cfg(not(unix))]
    let bytes = owned.as_slice();

    if !bytes.starts_with(b"/") {
        return Err("file URI path must be absolute".into());
    }
    let mut uri = String::from("file://");
    for &byte in bytes {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            uri.push(byte as char);
        } else {
            use std::fmt::Write as _;
            write!(&mut uri, "%{byte:02X}").expect("writing to String cannot fail");
        }
    }
    Ok(uri)
}

fn unsupported(platform: &'static str) -> Unsupported {
    Unsupported {
        platform,
        reason: "native file drag is currently available on Linux",
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn uri_offer_escapes_delimiters_and_requires_absolute_paths() {
        assert_eq!(
            uri_list(&[PathBuf::from("/tmp/music #1.wav")]).unwrap(),
            b"file:///tmp/music%20%231.wav\r\n"
        );
        assert!(uri_list(&[]).is_err());
        assert!(uri_list(&[PathBuf::from("relative")]).is_err());
    }
}
