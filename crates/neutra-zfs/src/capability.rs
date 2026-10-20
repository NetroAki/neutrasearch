//! Pass only the helper's ZFS read capabilities to the native reader process.
//!
//! This runs in `CommandExt::pre_exec`; keep it limited to Linux syscalls and
//! stack data because the child is between `fork` and `exec`.
use std::io;

const LINUX_CAPABILITY_VERSION_3: u32 = 0x2008_0522;
const SYS_ADMIN: u32 = 21;
const DAC_READ_SEARCH: u32 = 2;
const PR_CAP_AMBIENT: libc::c_int = 47;
const PR_CAP_AMBIENT_RAISE: libc::c_int = 2;
const PR_CAP_AMBIENT_CLEAR_ALL: libc::c_int = 4;

#[repr(C)]
struct CapHeader {
    version: u32,
    pid: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CapData {
    effective: u32,
    permitted: u32,
    inheritable: u32,
}

const fn cap_mask(capability: u32) -> (usize, u32) {
    ((capability / 32) as usize, 1 << (capability % 32))
}

/// Arrange for just CAP_DAC_READ_SEARCH and CAP_SYS_ADMIN to survive exec.
/// The parent helper's permitted set is the source of authority. If either
/// capability is absent, leave the child unchanged so unprivileged version
/// probes continue to work; an actual metadata scan then receives the normal
/// kernel permission error from the reader.
pub(super) fn inherit_zfs_capabilities() -> io::Result<()> {
    // SAFETY: geteuid only reads the current process credentials.
    if unsafe { libc::geteuid() } == 0 {
        return Ok(());
    }
    let mut header = CapHeader {
        version: LINUX_CAPABILITY_VERSION_3,
        pid: 0,
    };
    let mut data = [
        CapData {
            effective: 0,
            permitted: 0,
            inheritable: 0,
        },
        CapData {
            effective: 0,
            permitted: 0,
            inheritable: 0,
        },
    ];
    // SAFETY: these syscalls receive valid pointers to the Linux UAPI structs.
    unsafe {
        if libc::syscall(libc::SYS_capget, &mut header, data.as_mut_ptr()) != 0 {
            return Err(io::Error::last_os_error());
        }
        if libc::prctl(PR_CAP_AMBIENT, PR_CAP_AMBIENT_CLEAR_ALL, 0, 0, 0) != 0 {
            return Err(io::Error::last_os_error());
        }
    }

    let required = [DAC_READ_SEARCH, SYS_ADMIN];
    if required.iter().any(|capability| {
        let (word, mask) = cap_mask(*capability);
        data[word].permitted & mask == 0
    }) {
        return Ok(());
    }
    for word in &mut data {
        word.permitted = 0;
        word.inheritable = 0;
        word.effective = 0;
    }
    for capability in required {
        let (word, mask) = cap_mask(capability);
        data[word].permitted |= mask;
        data[word].inheritable |= mask;
    }
    // SAFETY: structs remain live through each syscall; arguments are fixed
    // Linux UAPI capability identifiers and prctl operation values.
    unsafe {
        if libc::syscall(libc::SYS_capset, &header, data.as_ptr()) != 0 {
            return Err(io::Error::last_os_error());
        }
        for capability in required {
            if libc::prctl(PR_CAP_AMBIENT, PR_CAP_AMBIENT_RAISE, capability, 0, 0) != 0 {
                return Err(io::Error::last_os_error());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_capabilities_share_word_without_losing_either_bit() {
        let mut data = [
            CapData {
                effective: 0,
                permitted: 0,
                inheritable: 0,
            },
            CapData {
                effective: 0,
                permitted: 0,
                inheritable: 0,
            },
        ];
        for capability in [DAC_READ_SEARCH, SYS_ADMIN] {
            let (word, mask) = cap_mask(capability);
            data[word].permitted |= mask;
            data[word].inheritable |= mask;
        }
        assert_eq!(data[0].permitted, (1 << DAC_READ_SEARCH) | (1 << SYS_ADMIN));
        assert_eq!(data[0].inheritable, data[0].permitted);
        assert_eq!(data[1].permitted, 0);
        assert_eq!(data[1].inheritable, 0);
    }
}
