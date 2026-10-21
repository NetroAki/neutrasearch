use super::*;
use std::collections::HashSet;

impl FanotifyWatcher {
    pub(super) fn collect_named(
        &self,
        event: &ParsedEvent,
        changes: &mut BTreeMap<String, DeltaChange>,
    ) -> Result<bool> {
        for mut location in event
            .locations
            .iter()
            .filter(|location| location.name.is_some())
            .cloned()
        {
            if self
                .native_root
                .as_ref()
                .map(|root| root.accepts_handle(&location.handle))
                .transpose()?
                .is_some_and(|accepted| !accepted)
            {
                continue;
            }
            let resolved = self.resolve_named(&mut location);
            let (path, stat, parent) = match resolved {
                Ok(resolved) => resolved,
                // A vanished parent is a directory removal covered by PreparedSweep.
                Err(error) if stale(&error) && self.native_root.is_some() => {
                    tracing::debug!(
                        "vanished Btrfs event parent deferred to metadata reconciliation"
                    );
                    continue;
                }
                Err(error) if stale(&error) => return Ok(true),
                Err(error) => return Err(error.into()),
            };
            let path = match self.visible_path(path) {
                Some(path) => path,
                None => continue,
            };
            if path.to_str().is_none() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "fanotify event path is not valid UTF-8",
                )
                .into());
            }
            if self.is_excluded(&path) {
                continue;
            }
            let remove = location.role == Role::Old
                || (location.role == Role::Current
                    && event.mask & (libc::FAN_DELETE | libc::FAN_MOVED_FROM) != 0);
            if remove {
                insert_remove(changes, &path);
            } else if location.role == Role::New
                || event.mask
                    & (libc::FAN_CREATE
                        | libc::FAN_MOVED_TO
                        | libc::FAN_ATTRIB
                        | libc::FAN_CLOSE_WRITE
                        | libc::FAN_RENAME)
                    != 0
            {
                if let Some(stat) = stat {
                    let record = make_record(&path, &stat, parent, &self.mount, self.source)
                        .ok_or_else(|| {
                            io::Error::new(
                                io::ErrorKind::InvalidData,
                                "fanotify event path is not valid UTF-8",
                            )
                        })?;
                    insert_upsert(changes, record);
                } else {
                    insert_remove(changes, &path);
                }
            }
        }
        Ok(false)
    }

    pub(super) fn collect_object(
        &self,
        event: ParsedEvent,
        changes: &mut BTreeMap<String, DeltaChange>,
        covered_deletions: &HashSet<Vec<u8>>,
    ) -> Result<bool> {
        if event.mask & (libc::FAN_ATTRIB | libc::FAN_CLOSE_WRITE) == 0 {
            return Ok(false);
        }
        if let Some(mut location) = event
            .locations
            .into_iter()
            .find(|location| location.role == Role::Object)
        {
            if self
                .native_root
                .as_ref()
                .map(|root| root.accepts_handle(&location.handle))
                .transpose()?
                .is_some_and(|accepted| !accepted)
            {
                return Ok(false);
            }
            match self.resolve_object(&mut location) {
                Ok((path, stat, parent)) => {
                    if let Some(path) = self
                        .visible_path(path)
                        .filter(|path| !self.is_excluded(path))
                    {
                        if path.to_str().is_none() {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "fanotify event path is not valid UTF-8",
                            )
                            .into());
                        }
                        if stat.st_nlink == 0 {
                            insert_remove(changes, &path);
                        } else {
                            let record =
                                make_record(&path, &stat, parent, &self.mount, self.source)
                                    .ok_or_else(|| {
                                        io::Error::new(
                                            io::ErrorKind::InvalidData,
                                            "fanotify event path is not valid UTF-8",
                                        )
                                    })?;
                            insert_upsert(changes, record);
                        }
                    }
                }
                Err(error) if stale(&error) && covered_deletions.contains(&location.handle) => {
                    return Ok(false);
                }
                // Btrfs reconciliation covers vanished objects.
                Err(error) if stale(&error) && self.native_root.is_some() => return Ok(false),
                Err(error) if stale(&error) => return Ok(true),
                Err(error) => return Err(error.into()),
            }
        }
        Ok(false)
    }
}
