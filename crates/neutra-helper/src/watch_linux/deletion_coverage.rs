use super::*;
use std::collections::HashSet;
use std::collections::VecDeque;

const MAX_HANDLES: usize = 4096;
const MAX_BYTES: usize = 1024 * 1024;

#[derive(Default)]
pub(super) struct DeletedHandles {
    handles: HashSet<Vec<u8>>,
    order: VecDeque<Vec<u8>>,
    bytes: usize,
}

impl DeletedHandles {
    pub(super) fn extend(&mut self, handles: impl IntoIterator<Item = Vec<u8>>) {
        let incoming: Vec<_> = handles
            .into_iter()
            .filter(|handle| !self.handles.contains(handle))
            .collect();
        for handle in incoming {
            if handle.len() > MAX_BYTES || self.handles.contains(&handle) {
                continue;
            }
            self.bytes += handle.len();
            self.handles.insert(handle.clone());
            self.order.push_back(handle);
            while self.order.len() > MAX_HANDLES || self.bytes > MAX_BYTES {
                if let Some(old) = self.order.pop_front() {
                    self.bytes -= old.len();
                    self.handles.remove(&old);
                }
            }
        }
    }

    pub(super) fn handles(&self) -> &HashSet<Vec<u8>> {
        &self.handles
    }
}

pub(super) fn insert_removal(
    changes: &mut BTreeMap<String, DeltaChange>,
    path: &Path,
    mask: u64,
) -> io::Result<()> {
    if mask & libc::FAN_ONDIR == 0 {
        return insert_remove(changes, path);
    }
    let path = path.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "fanotify event path is not valid UTF-8",
        )
    })?;
    changes.retain(|_, change| {
        let existing = match change {
            DeltaChange::Upsert(record) => &record.path,
            DeltaChange::Remove(path) | DeltaChange::RemovePrefix(path) => path,
        };
        !neutra_core::paths::path_in_portable_root(existing, path)
    });
    changes.insert(format!("\0{path}"), DeltaChange::RemovePrefix(path.into()));
    Ok(())
}

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
    let mut ancestors = watcher.deleted_handles.handles().clone();
    ancestors.extend(covered.iter().cloned());
    inherit_deletions(events, &mut ancestors);
    Ok(ancestors)
}

fn inherit_deletions(events: &[ParsedEvent], covered: &mut HashSet<Vec<u8>>) {
    let mut children = std::collections::HashMap::<Vec<u8>, Vec<Vec<u8>>>::new();
    for event in events {
        let Some(target) = named_delete_target(event) else {
            continue;
        };
        for parent in event
            .locations
            .iter()
            .filter(|location| location.role == Role::Current && location.name.is_some())
        {
            children
                .entry(parent.handle.clone())
                .or_default()
                .push(target.clone());
        }
    }
    let mut pending: Vec<_> = covered.iter().cloned().collect();
    while let Some(parent) = pending.pop() {
        for child in children.remove(&parent).into_iter().flatten() {
            if covered.insert(child.clone()) {
                pending.push(child);
            }
        }
    }
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

    #[test]
    fn deleted_handle_evidence_survives_batches_and_is_bounded() {
        let mut cache = DeletedHandles::default();
        cache.extend(HashSet::from([vec![1, 2, 3]]));
        cache.extend(HashSet::new());
        assert!(cache.handles().contains(&vec![1, 2, 3]));
        assert!(!cache.handles().contains(&vec![1, 2, 4]));
        for id in 0..MAX_HANDLES as u64 {
            cache.extend(HashSet::from([id.to_ne_bytes().to_vec()]));
        }
        assert_eq!(cache.handles().len(), MAX_HANDLES);
        assert!(!cache.handles().contains(&vec![1, 2, 3]));
        cache.extend(HashSet::from([vec![42; MAX_BYTES + 1]]));
        assert_eq!(cache.handles().len(), MAX_HANDLES);
        for id in 0..5 {
            cache.extend(HashSet::from([vec![id; MAX_BYTES / 4]]));
        }
        assert!(cache.bytes <= MAX_BYTES);
    }

    #[test]
    fn refreshing_history_cannot_evict_new_deletion_evidence() {
        let history: Vec<_> = (0..MAX_HANDLES as u64)
            .map(|id| id.to_ne_bytes().to_vec())
            .collect();
        let mut cache = DeletedHandles::default();
        cache.extend(history.clone());
        let fresh = u64::MAX.to_ne_bytes().to_vec();
        cache.extend(std::iter::once(fresh.clone()).chain(history));
        assert!(cache.handles().contains(&fresh));
        assert_eq!(cache.handles().len(), MAX_HANDLES);
    }

    #[test]
    fn removed_folder_drops_earlier_children_but_preserves_siblings_and_recreation() {
        let mut changes = BTreeMap::new();
        insert_remove(&mut changes, Path::new("/d/old")).unwrap();
        insert_remove(&mut changes, Path::new("/d2/kept")).unwrap();
        insert_removal(
            &mut changes,
            Path::new("/d"),
            libc::FAN_DELETE | libc::FAN_ONDIR,
        )
        .unwrap();
        assert!(!changes.contains_key("/d/old"));
        assert!(changes.contains_key("/d2/kept"));
        assert!(
            matches!(changes.get("\0/d"), Some(DeltaChange::RemovePrefix(path)) if &**path == "/d")
        );
        insert_remove(&mut changes, Path::new("/d/new")).unwrap();
        assert!(changes.contains_key("/d/new"));
        assert!(matches!(
            changes.values().next(),
            Some(DeltaChange::RemovePrefix(_))
        ));
    }

    #[test]
    fn deleted_ancestor_covers_descendant_handles_in_any_event_order() {
        let leaf = vec![1];
        let parent = vec![2];
        let root = vec![3];
        let deletion = |target: &[u8], parent: &[u8]| {
            event(
                libc::FAN_DELETE,
                vec![
                    object(target),
                    Location {
                        role: Role::Current,
                        handle: parent.to_vec(),
                        name: Some("gone".into()),
                    },
                ],
            )
        };
        let events = [deletion(&leaf, &parent), deletion(&parent, &root)];
        let mut covered = HashSet::from([root]);
        inherit_deletions(&events, &mut covered);
        assert!(covered.contains(&leaf));
        assert!(covered.contains(&parent));
        assert!(!covered.contains(&vec![4]));
    }

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
