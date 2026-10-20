use super::super::state::wide;
use anyhow::{Context, Result};
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    ptr::null_mut,
};
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, GetNamedSecurityInfoW, SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    AclSizeInformation, GetAce, GetAclInformation, ACCESS_ALLOWED_ACE, ACL_SIZE_INFORMATION,
    DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
};

pub(in crate::windows_service) fn validate_installed_service_files() -> Result<()> {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    let helper = std::env::current_exe().context("resolve service executable")?;
    let directory = helper
        .parent()
        .context("service executable has no installation directory")?;
    let program_files = std::env::var_os("ProgramFiles")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .context("ProgramFiles is unavailable")?;
    if !windows_path_is_under(directory, &program_files) {
        anyhow::bail!(
            "privileged scanner is outside Program Files: {}",
            directory.display()
        );
    }

    let gui = directory.join("neutrasearch.exe");
    for file in [&helper, &gui] {
        let metadata = std::fs::symlink_metadata(file)
            .with_context(|| format!("inspect installed service file {}", file.display()))?;
        if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            anyhow::bail!(
                "privileged scanner file must be regular and not a reparse point: {}",
                file.display()
            );
        }
    }

    let mut ancestor = Some(directory);
    while let Some(path) = ancestor {
        let metadata = std::fs::symlink_metadata(path)
            .with_context(|| format!("inspect service directory {}", path.display()))?;
        if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            anyhow::bail!(
                "privileged scanner directory must not contain reparse points: {}",
                path.display()
            );
        }
        if windows_path_eq(path, &program_files) {
            break;
        }
        ancestor = path.parent();
    }
    for path in [directory, helper.as_path(), gui.as_path()] {
        reject_non_admin_write_acl(path)?;
    }
    Ok(())
}

fn reject_non_admin_write_acl(path: &Path) -> Result<()> {
    const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
    const WRITE_MASK: u32 = 0x0000_0002 // FILE_WRITE_DATA / FILE_ADD_FILE
        | 0x0000_0004 // FILE_APPEND_DATA / FILE_ADD_SUBDIRECTORY
        | 0x0000_0010 // FILE_WRITE_EA
        | 0x0000_0040 // FILE_DELETE_CHILD
        | 0x0000_0100 // FILE_WRITE_ATTRIBUTES
        | 0x0001_0000 // DELETE
        | 0x0004_0000 // WRITE_DAC
        | 0x0008_0000 // WRITE_OWNER
        | 0x1000_0000 // GENERIC_ALL
        | 0x4000_0000; // GENERIC_WRITE

    let name = wide(&path.to_string_lossy());
    let mut dacl = null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    let status = unsafe {
        GetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 {
        anyhow::bail!(
            "read ACL for {} failed with Windows error {status}",
            path.display()
        );
    }
    let result = (|| -> Result<()> {
        if dacl.is_null() {
            anyhow::bail!("refusing unprotected null DACL on {}", path.display());
        }
        let mut information = ACL_SIZE_INFORMATION::default();
        if unsafe {
            GetAclInformation(
                dacl,
                (&mut information as *mut ACL_SIZE_INFORMATION).cast(),
                std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
                AclSizeInformation,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error()).context("read ACL entry count");
        }
        for index in 0..information.AceCount {
            let mut raw_ace = null_mut();
            if unsafe { GetAce(dacl, index, &mut raw_ace) } == 0 || raw_ace.is_null() {
                return Err(std::io::Error::last_os_error()).context("read ACL entry");
            }
            let ace = unsafe { &*(raw_ace as *const ACCESS_ALLOWED_ACE) };
            // The installer emits only ordinary allow ACEs. Unknown/object or
            // callback ACEs are rejected rather than parsed optimistically.
            if ace.Header.AceType != ACCESS_ALLOWED_ACE_TYPE {
                anyhow::bail!("unexpected ACL entry type on {}", path.display());
            }
            if ace.Mask & WRITE_MASK == 0 {
                continue;
            }
            let sid = (&ace.SidStart as *const u32).cast_mut().cast();
            let sid = sid_string(sid)?;
            if !matches!(sid.as_str(), "S-1-5-18" | "S-1-5-32-544") {
                anyhow::bail!(
                    "non-administrator SID {sid} can modify privileged scanner path {}",
                    path.display()
                );
            }
        }
        Ok(())
    })();
    unsafe {
        LocalFree(descriptor);
    }
    result
}

fn sid_string(sid: windows_sys::Win32::Security::PSID) -> Result<String> {
    let mut text = null_mut();
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 || text.is_null() {
        return Err(std::io::Error::last_os_error()).context("render ACL SID");
    }
    let mut length = 0usize;
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    let value = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) });
    unsafe {
        LocalFree(text.cast());
    }
    Ok(value)
}

pub(super) fn validate_client_path(helper: &Path, client: &Path) -> Result<()> {
    let expected_directory = helper
        .parent()
        .context("installed helper has no parent directory")?;
    let client_directory = client
        .parent()
        .context("pipe client has no parent directory")?;
    let filename = client
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or_default();
    if !filename.eq_ignore_ascii_case("neutrasearch.exe")
        || !windows_path_eq(expected_directory, client_directory)
    {
        anyhow::bail!(
            "client must be the installed Neutrasearch GUI beside the service helper (got {})",
            client.display()
        );
    }
    Ok(())
}
fn windows_path_eq(left: &Path, right: &Path) -> bool {
    normalize_windows_path(left) == normalize_windows_path(right)
}

fn windows_path_is_under(path: &Path, root: &Path) -> bool {
    let path = normalize_windows_path(path);
    let root = normalize_windows_path(root);
    path.strip_prefix(&root)
        .is_some_and(|tail| tail.starts_with('\\'))
}

fn normalize_windows_path(path: &Path) -> String {
    path.to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_installed_gui_beside_the_helper_is_trusted() {
        let helper = Path::new(r"C:\Program Files\Neutrasearch\neutrasearch-helper.exe");
        assert!(validate_client_path(
            helper,
            Path::new(r"c:\program files\neutrasearch\Neutrasearch.exe")
        )
        .is_ok());
        assert!(validate_client_path(helper, Path::new(r"C:\Temp\neutrasearch.exe")).is_err());
        assert!(validate_client_path(
            helper,
            Path::new(r"C:\Program Files\Neutrasearch\neutrasearch-mcp.exe")
        )
        .is_err());
        assert!(windows_path_is_under(
            Path::new(r"C:\Program Files\Neutrasearch"),
            Path::new(r"C:\Program Files")
        ));
        assert!(!windows_path_is_under(
            Path::new(r"C:\Program Files-old\Neutrasearch"),
            Path::new(r"C:\Program Files")
        ));
    }
}
