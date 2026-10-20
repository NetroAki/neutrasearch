use std::{
    ffi::OsString,
    fs::OpenOptions,
    io::Read,
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Component, Path, PathBuf},
};

pub(super) fn encode(path: &Path) -> Result<String, String> {
    let encoded: String = path
        .as_os_str()
        .as_bytes()
        .iter()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"/-._~".contains(b) {
                (*b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    Ok(format!(
        "[Trash Info]\nPath={encoded}\nDeletionDate={}\n",
        deletion_date()?
    ))
}

fn deletion_date() -> Result<String, String> {
    let mut local = std::mem::MaybeUninit::<libc::tm>::uninit();
    let mut bytes = [0u8; 32];
    let written = unsafe {
        let now = libc::time(std::ptr::null_mut());
        if libc::localtime_r(&now, local.as_mut_ptr()).is_null() {
            return Err("cannot record Trash deletion date".into());
        }
        libc::strftime(
            bytes.as_mut_ptr().cast(),
            bytes.len(),
            c"%Y-%m-%dT%H:%M:%S".as_ptr(),
            local.as_ptr(),
        )
    };
    if written == 0 {
        return Err("cannot record Trash deletion date".into());
    }
    Ok(String::from_utf8_lossy(&bytes[..written]).into_owned())
}

pub(super) fn read(token: &Path) -> Result<PathBuf, String> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(token)
        .map_err(|e| e.to_string())?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::getuid() }
        || metadata.nlink() != 1
        || metadata.mode() & 0o077 != 0
    {
        return Err("unsafe Trash restore information".into());
    }
    let mut text = String::new();
    file.take(65_537)
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    if text.len() > 65_536 || !text.starts_with("[Trash Info]\n") {
        return Err("invalid Trash restore information".into());
    }
    let mut paths = text.lines().filter_map(|s| s.strip_prefix("Path="));
    let path = decode(paths.next().ok_or("Trash original path is missing")?)?;
    if paths.next().is_some()
        || path.as_os_str().is_empty()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err("invalid Trash original path".into());
    }
    Ok(path)
}

fn decode(value: &str) -> Result<PathBuf, String> {
    let mut result = Vec::new();
    let mut bytes = value.as_bytes().iter().copied();
    while let Some(byte) = bytes.next() {
        let decoded = if byte == b'%' {
            let hi = bytes.next().and_then(|c| (c as char).to_digit(16));
            let lo = bytes.next().and_then(|c| (c as char).to_digit(16));
            let (hi, lo) = hi.zip(lo).ok_or("invalid Trash path escape")?;
            (hi * 16 + lo) as u8
        } else {
            byte
        };
        if decoded == 0 {
            return Err("invalid Trash original path".into());
        }
        result.push(decoded);
    }
    Ok(PathBuf::from(OsString::from_vec(result)))
}
