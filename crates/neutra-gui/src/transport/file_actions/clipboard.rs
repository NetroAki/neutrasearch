#[cfg(all(test, unix))]
use std::path::Path;
use std::{
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

type FileClipboard = Option<(bool, Vec<PathBuf>)>;
static FILE_CLIPBOARD: OnceLock<Mutex<FileClipboard>> = OnceLock::new();
static DESKTOP_CLIPBOARD: OnceLock<Mutex<Option<arboard::Clipboard>>> = OnceLock::new();

fn file_clipboard() -> &'static Mutex<Option<(bool, Vec<PathBuf>)>> {
    FILE_CLIPBOARD.get_or_init(|| Mutex::new(None))
}

pub(super) fn set_clipboard(paths: &[PathBuf], cut: bool) -> Result<(), String> {
    if paths.is_empty() {
        return Err("no files selected".into());
    }
    let mut owner = DESKTOP_CLIPBOARD
        .get_or_init(|| Mutex::new(None))
        .lock()
        .map_err(|_| "clipboard is unavailable")?;
    if owner.is_none() {
        *owner = Some(
            arboard::Clipboard::new()
                .map_err(|e| format!("cannot access desktop clipboard: {e}"))?,
        );
    }
    owner
        .as_mut()
        .expect("created above")
        .set()
        .file_list(paths)
        .map_err(|e| format!("cannot set file clipboard: {e}"))?;
    *file_clipboard()
        .lock()
        .map_err(|_| "file clipboard state is unavailable")? = Some((cut, paths.to_vec()));
    Ok(())
}

pub(super) fn get_clipboard() -> Result<(bool, Vec<PathBuf>), String> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| format!("cannot access desktop clipboard: {e}"))?;
    let paths = clipboard
        .get()
        .file_list()
        .map_err(|e| format!("clipboard does not contain file paths: {e}"))?;
    if paths.is_empty() {
        return Err("clipboard contains no file paths".into());
    }
    let cut = file_clipboard()
        .lock()
        .ok()
        .and_then(|state| {
            state
                .as_ref()
                .filter(|(_, previous)| *previous == paths)
                .map(|(cut, _)| *cut)
        })
        .unwrap_or(false);
    Ok((cut, paths))
}

#[cfg(all(test, unix))]
pub(super) fn gio_uri(p: &Path) -> Result<String, String> {
    Ok(format!("file://{}", percent_path(p)))
}

#[cfg(all(test, unix))]
fn percent_path(p: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    p.as_os_str()
        .as_bytes()
        .iter()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"/-._~".contains(b) {
                (*b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

#[cfg(all(test, unix))]
pub(super) fn url_to_path(s: &str) -> Result<PathBuf, String> {
    if !s.starts_with("file:///") {
        return Err("non-local URI".into());
    }
    let bytes = &s.as_bytes()[7..];
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err("bad URI escape".into());
            }
            out.push(
                u8::from_str_radix(
                    std::str::from_utf8(&bytes[i + 1..i + 3]).map_err(|e| e.to_string())?,
                    16,
                )
                .map_err(|e| e.to_string())?,
            );
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    use std::os::unix::ffi::OsStringExt;
    Ok(PathBuf::from(std::ffi::OsString::from_vec(out)))
}
