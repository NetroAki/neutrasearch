use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Role {
    Object,
    Current,
    Old,
    New,
}

#[derive(Clone, Debug)]
pub(super) struct Location {
    pub(super) role: Role,
    pub(super) handle: Vec<u8>,
    pub(super) name: Option<OsString>,
}

#[derive(Debug)]
pub(super) struct ParsedEvent {
    pub(super) mask: u64,
    pub(super) locations: Vec<Location>,
}

pub(super) fn parse_events(bytes: &[u8]) -> io::Result<Vec<ParsedEvent>> {
    let mut events = Vec::new();
    let mut event_at = 0usize;
    while event_at < bytes.len() {
        if bytes.len() - event_at < META_LEN {
            return Err(invalid("truncated fanotify event metadata"));
        }
        let event_len = u32_at(bytes, event_at)? as usize;
        let metadata_len = u16_at(bytes, event_at + 6)? as usize;
        if bytes[event_at + 4] != libc::FANOTIFY_METADATA_VERSION {
            return Err(invalid("unsupported fanotify metadata version"));
        }
        if metadata_len < META_LEN || event_len < metadata_len {
            return Err(invalid("invalid fanotify event lengths"));
        }
        let event_end = event_at
            .checked_add(event_len)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| invalid("fanotify event exceeds read buffer"))?;
        let mask = u64_at(bytes, event_at + 8)?;
        let mut locations = Vec::new();
        let mut info_at = event_at + metadata_len;
        while info_at < event_end {
            if event_end - info_at < INFO_HEADER_LEN {
                return Err(invalid("truncated fanotify info header"));
            }
            let info_type = bytes[info_at];
            let info_len = u16_at(bytes, info_at + 2)? as usize;
            if info_len < INFO_HEADER_LEN || info_at + info_len > event_end {
                return Err(invalid("invalid fanotify info length"));
            }
            if let Some(role) = role_for(info_type) {
                locations.push(parse_location(
                    &bytes[info_at..info_at + info_len],
                    role,
                    info_type != libc::FAN_EVENT_INFO_TYPE_FID
                        && info_type != libc::FAN_EVENT_INFO_TYPE_DFID,
                )?);
            }
            info_at += info_len;
        }
        events.push(ParsedEvent { mask, locations });
        event_at = event_end;
    }
    Ok(events)
}

pub(super) fn rescan_reason(events: &[ParsedEvent], atomic_rename: bool) -> Option<&'static str> {
    if events
        .iter()
        .any(|event| event.mask & libc::FAN_Q_OVERFLOW != 0)
    {
        return Some("fanotify queue overflowed");
    }
    if events.iter().any(|event| {
        event.mask & libc::FAN_ONDIR != 0 && event.mask & (libc::FAN_MOVE | libc::FAN_RENAME) != 0
    }) {
        return Some("directory rename requires descendant path reconciliation");
    }
    if !atomic_rename && events.iter().any(|event| event.mask & libc::FAN_MOVE != 0) {
        return Some("kernel reported an unpaired move event");
    }
    None
}

pub(super) fn validate_utf8_paths(events: &[ParsedEvent]) -> io::Result<()> {
    if events
        .iter()
        .flat_map(|event| &event.locations)
        .any(|location| {
            location
                .name
                .as_ref()
                .is_some_and(|name| name.to_str().is_none())
        })
    {
        return Err(invalid("fanotify event name is not valid UTF-8"));
    }
    Ok(())
}

fn role_for(info_type: u8) -> Option<Role> {
    match info_type {
        libc::FAN_EVENT_INFO_TYPE_FID => Some(Role::Object),
        libc::FAN_EVENT_INFO_TYPE_DFID | libc::FAN_EVENT_INFO_TYPE_DFID_NAME => Some(Role::Current),
        libc::FAN_EVENT_INFO_TYPE_OLD_DFID_NAME => Some(Role::Old),
        libc::FAN_EVENT_INFO_TYPE_NEW_DFID_NAME => Some(Role::New),
        _ => None,
    }
}

fn parse_location(info: &[u8], role: Role, has_name: bool) -> io::Result<Location> {
    if info.len() < FID_PREFIX_LEN + FILE_HANDLE_HEADER_LEN {
        return Err(invalid("fanotify FID record is too short"));
    }
    let handle_bytes = u32_at(info, FID_PREFIX_LEN)? as usize;
    let handle_end = FID_PREFIX_LEN
        .checked_add(FILE_HANDLE_HEADER_LEN)
        .and_then(|start| start.checked_add(handle_bytes))
        .filter(|end| *end <= info.len())
        .ok_or_else(|| invalid("fanotify file handle exceeds info record"))?;
    let handle = info[FID_PREFIX_LEN..handle_end].to_vec();
    let name = if has_name {
        let rest = &info[handle_end..];
        let nul = rest
            .iter()
            .position(|byte| *byte == 0)
            .ok_or_else(|| invalid("fanotify name is not NUL-terminated"))?;
        Some(OsString::from_vec(rest[..nul].to_vec()))
    } else {
        None
    };
    Ok(Location { role, handle, name })
}

fn u16_at(bytes: &[u8], at: usize) -> io::Result<u16> {
    let value = bytes
        .get(at..at + 2)
        .ok_or_else(|| invalid("truncated u16"))?;
    Ok(u16::from_ne_bytes(value.try_into().unwrap()))
}

fn u32_at(bytes: &[u8], at: usize) -> io::Result<u32> {
    let value = bytes
        .get(at..at + 4)
        .ok_or_else(|| invalid("truncated u32"))?;
    Ok(u32::from_ne_bytes(value.try_into().unwrap()))
}

fn u64_at(bytes: &[u8], at: usize) -> io::Result<u64> {
    let value = bytes
        .get(at..at + 8)
        .ok_or_else(|| invalid("truncated u64"))?;
    Ok(u64::from_ne_bytes(value.try_into().unwrap()))
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_name_and_handle_with_bounds() {
        let handle_payload = [0xaa, 0xbb];
        let mut info = Vec::new();
        info.extend_from_slice(&[libc::FAN_EVENT_INFO_TYPE_DFID_NAME, 0, 0, 0]);
        info.extend_from_slice(&[0; 8]);
        info.extend_from_slice(&(handle_payload.len() as u32).to_ne_bytes());
        info.extend_from_slice(&7i32.to_ne_bytes());
        info.extend_from_slice(&handle_payload);
        info.extend_from_slice(b"report.txt\0");
        let info_len = info.len() as u16;
        info[2..4].copy_from_slice(&info_len.to_ne_bytes());

        let event_len = META_LEN + info.len();
        let mut event = vec![0; META_LEN];
        event[0..4].copy_from_slice(&(event_len as u32).to_ne_bytes());
        event[4] = libc::FANOTIFY_METADATA_VERSION;
        event[6..8].copy_from_slice(&(META_LEN as u16).to_ne_bytes());
        event[8..16].copy_from_slice(&libc::FAN_CREATE.to_ne_bytes());
        event.extend_from_slice(&info);

        let parsed = parse_events(&event).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].locations[0].role, Role::Current);
        assert_eq!(
            parsed[0].locations[0].name.as_deref(),
            Some(std::ffi::OsStr::new("report.txt"))
        );
        assert_eq!(parsed[0].locations[0].handle.len(), 10);
    }

    #[test]
    fn invalid_byte_event_name_is_rejected_but_replacement_character_is_valid() {
        fn event_for(name: &[u8]) -> Vec<u8> {
            let mut info = Vec::new();
            info.extend_from_slice(&[libc::FAN_EVENT_INFO_TYPE_DFID_NAME, 0, 0, 0]);
            info.extend_from_slice(&[0; 8]);
            info.extend_from_slice(&0u32.to_ne_bytes());
            info.extend_from_slice(&0i32.to_ne_bytes());
            info.extend_from_slice(name);
            info.push(0);
            let len = info.len() as u16;
            info[2..4].copy_from_slice(&len.to_ne_bytes());
            let mut event = vec![0; META_LEN];
            event[0..4].copy_from_slice(&((META_LEN + info.len()) as u32).to_ne_bytes());
            event[4] = libc::FANOTIFY_METADATA_VERSION;
            event[6..8].copy_from_slice(&(META_LEN as u16).to_ne_bytes());
            event.extend(info);
            event
        }
        let invalid = parse_events(&event_for(b"bad\xffname")).unwrap();
        assert_eq!(
            validate_utf8_paths(&invalid).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        let valid = parse_events(&event_for("valid�name".as_bytes())).unwrap();
        validate_utf8_paths(&valid).unwrap();
        assert_eq!(
            valid[0].locations[0].name.as_ref().unwrap().to_str(),
            Some("valid�name")
        );
    }

    #[test]
    fn directory_renames_require_native_reindex() {
        let events = [ParsedEvent {
            mask: libc::FAN_RENAME | libc::FAN_ONDIR,
            locations: Vec::new(),
        }];
        assert_eq!(
            rescan_reason(&events, true),
            Some("directory rename requires descendant path reconciliation")
        );
    }

    #[test]
    fn legacy_unpaired_moves_require_native_reindex() {
        let events = [ParsedEvent {
            mask: libc::FAN_MOVED_FROM,
            locations: Vec::new(),
        }];
        assert_eq!(
            rescan_reason(&events, false),
            Some("kernel reported an unpaired move event")
        );
    }

    #[test]
    fn rejects_info_record_past_event_boundary() {
        let mut event = vec![0; META_LEN + INFO_HEADER_LEN];
        let len = event.len() as u32;
        event[0..4].copy_from_slice(&len.to_ne_bytes());
        event[4] = libc::FANOTIFY_METADATA_VERSION;
        event[6..8].copy_from_slice(&(META_LEN as u16).to_ne_bytes());
        event[META_LEN + 2..META_LEN + 4].copy_from_slice(&99u16.to_ne_bytes());
        assert!(parse_events(&event).is_err());
    }
}
