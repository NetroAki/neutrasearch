use super::*;
use std::collections::HashSet;

pub(super) fn confirmed_named_deletions(
    watcher: &FanotifyWatcher,
    events: &[ParsedEvent],
) -> Result<HashSet<Vec<u8>>> {
    let mut covered = HashSet::new();
    for event in events {
        let Some(target_handle) = named_delete_target(event) else {
            continue;
        };
        for mut location in event
            .locations
            .iter()
            .filter(|location| location.role == Role::Current && location.name.is_some())
            .cloned()
        {
            if watcher
                .native_root
                .as_ref()
                .map(|root| root.accepts_handle(&location.handle))
                .transpose()?
                .is_some_and(|accepted| !accepted)
            {
                continue;
            }
            let (path, _, _) = match watcher.resolve_named(&mut location) {
                Ok(resolved) => resolved,
                Err(error) if stale(&error) => continue,
                Err(error) => return Err(error.into()),
            };
            let Some(path) = watcher.visible_path(path) else {
                continue;
            };
            if !watcher.is_excluded(&path) {
                covered.insert(target_handle.clone());
                break;
            }
        }
    }
    Ok(covered)
}

fn named_delete_target(event: &ParsedEvent) -> Option<Vec<u8>> {
    if event.mask & libc::FAN_DELETE == 0 {
        return None;
    }
    let target = event
        .locations
        .iter()
        .find(|location| location.role == Role::Object)?;
    event
        .locations
        .iter()
        .any(|location| location.role == Role::Current && location.name.is_some())
        .then(|| target.handle.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn event(mask: u64, locations: Vec<Location>) -> ParsedEvent {
        ParsedEvent { mask, locations }
    }

    fn object(handle: &[u8]) -> Location {
        Location {
            role: Role::Object,
            handle: handle.to_vec(),
            name: None,
        }
    }

    fn named_current() -> Location {
        Location {
            role: Role::Current,
            handle: vec![9, 8],
            name: Some(OsString::from("removed.txt")),
        }
    }

    #[test]
    fn coverage_candidate_requires_delete_exact_target_and_named_current_record() {
        let target = vec![1, 2, 3];
        let deletion = event(libc::FAN_DELETE, vec![object(&target), named_current()]);
        assert_eq!(named_delete_target(&deletion), Some(target));

        let create = event(libc::FAN_CREATE, vec![object(&[1, 2, 3]), named_current()]);
        assert_eq!(named_delete_target(&create), None);

        let no_target = event(libc::FAN_DELETE, vec![named_current()]);
        assert_eq!(named_delete_target(&no_target), None);

        let no_name = event(
            libc::FAN_DELETE,
            vec![
                object(&[1, 2, 3]),
                Location {
                    role: Role::Current,
                    handle: vec![9, 8],
                    name: None,
                },
            ],
        );
        assert_eq!(named_delete_target(&no_name), None);

        let old_name = event(
            libc::FAN_DELETE,
            vec![
                object(&[1, 2, 3]),
                Location {
                    role: Role::Old,
                    handle: vec![9, 8],
                    name: Some(OsString::from("removed.txt")),
                },
            ],
        );
        assert_eq!(named_delete_target(&old_name), None);
    }
}
