use std::{io, path::Path};

#[cfg(windows)]
mod windows_replace {
    use std::{io, os::windows::ffi::OsStrExt, path::Path};

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    #[link(name = "kernel32")]
    extern "system" {
        fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
    }

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    fn move_file(from: &Path, to: &Path, flags: u32) -> io::Result<()> {
        let (from, to) = (wide(from), wide(to));
        // SAFETY: both buffers are NUL-terminated and outlive the call.
        if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), flags) } == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    /// Replace `path` with `temp`. Windows refuses to overwrite a file that a
    /// reader has mapped, but it does allow renaming one, so the mapped file is
    /// moved aside under a bounded set of names and the readers keep their view
    /// until they reopen. An aside file that is no longer mapped is deleted the
    /// next time a replacement runs.
    pub(crate) fn replace_file(temp: &Path, path: &Path) -> io::Result<()> {
        let flags = MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH;
        let first = move_file(temp, path, flags);
        if first.is_ok() || !path.exists() {
            return first;
        }
        let mut aside = None;
        for slot in 0..8 {
            let mut name = path.as_os_str().to_owned();
            name.push(format!(".replaced{slot}"));
            let candidate = std::path::PathBuf::from(name);
            if !candidate.exists() || std::fs::remove_file(&candidate).is_ok() {
                aside = Some(candidate);
                break;
            }
        }
        let Some(aside) = aside else {
            return first;
        };
        move_file(path, &aside, 0)?;
        if let Err(error) = move_file(temp, path, flags) {
            let _ = move_file(&aside, path, 0);
            return Err(error);
        }
        let _ = std::fs::remove_file(&aside);
        Ok(())
    }
}
#[cfg(windows)]
pub(crate) use windows_replace::replace_file;

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn replaces_a_file_while_a_reader_has_it_mapped() {
        let dir = std::env::temp_dir().join(format!("ns-replace-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (path, temp) = (dir.join("base.nsx"), dir.join("base.nsx.tmp"));
        std::fs::File::create(&path)
            .unwrap()
            .write_all(b"old bytes")
            .unwrap();
        let file = std::fs::File::open(&path).unwrap();
        let map = unsafe { memmap2::Mmap::map(&file).unwrap() };
        std::fs::File::create(&temp)
            .unwrap()
            .write_all(b"new bytes")
            .unwrap();
        replace_file(&temp, &path).unwrap();
        assert_eq!(&map[..], b"old bytes");
        assert_eq!(std::fs::read(&path).unwrap(), b"new bytes");
        drop(map);
        drop(file);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(not(unix))]
pub(crate) fn sync_parent(_path: &Path) -> io::Result<()> {
    Ok(())
}
