use super::*;

impl FanotifyWatcher {
    pub fn read_batch(&mut self) -> Result<WatchBatch> {
        let mut events = self.read_events()?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(50);
        for attempt in 0..8 {
            validate_utf8_paths(&events)?;
            if let Some(reason) = rescan_reason(&events, self.atomic_rename) {
                return Ok(self.coverage_gap(reason));
            }
            let covered = deletion_coverage::confirmed_named_deletions(self, &events)?;
            let mut changes = BTreeMap::<String, DeltaChange>::new();
            let mut unresolved = false;
            for event in &events {
                if self.collect_event(event, &mut changes, &covered)? {
                    unresolved = true;
                    break;
                }
            }
            if !unresolved {
                self.deleted_handles.extend(covered);
                return Ok(WatchBatch::Changes(changes.into_values().collect()));
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if attempt == 7 || remaining.is_zero() || !self.wait_readable(remaining)? {
                break;
            }
            events.extend(self.read_events()?);
        }
        tracing::error!(mount = %self.mount.mountpoint.display(), fs = %self.mount.fs.label(), "unresolved native event has no confirmed deletion coverage");
        Ok(self.coverage_gap("an event path became unresolvable before delivery"))
    }

    fn coverage_gap(&self, reason: &'static str) -> WatchBatch {
        if self.mount.fs == neutra_core::FsKind::Btrfs {
            WatchBatch::ReconcileRequired(reason)
        } else {
            WatchBatch::RescanRequired(reason)
        }
    }

    fn read_events(&mut self) -> Result<Vec<ParsedEvent>> {
        let bytes = loop {
            match self.events.read(&mut self.buffer) {
                Ok(bytes) => break bytes,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error.into()),
            }
        };
        if bytes == 0 {
            return Err(
                io::Error::new(io::ErrorKind::UnexpectedEof, "fanotify descriptor closed").into(),
            );
        }
        Ok(parse_events(&self.buffer[..bytes])?)
    }

    /// Block until events are readable or the timeout elapses. Returns true
    /// when a subsequent read_batch will not block. Used to coalesce event
    /// bursts before committing them.
    pub fn wait_readable(&self, timeout: std::time::Duration) -> io::Result<bool> {
        let mut fds = [libc::pollfd {
            fd: self.events.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        }];
        loop {
            let ready = unsafe { libc::poll(fds.as_mut_ptr(), 1, timeout.as_millis() as i32) };
            if ready < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            return Ok(ready > 0);
        }
    }

    /// Returns true when a raced/deleted handle makes this batch ambiguous.
    fn collect_event(
        &self,
        event: &ParsedEvent,
        changes: &mut BTreeMap<String, DeltaChange>,
        covered_deletions: &std::collections::HashSet<Vec<u8>>,
    ) -> Result<bool> {
        if event
            .locations
            .iter()
            .any(|location| location.name.is_some())
        {
            self.collect_named(event, changes, covered_deletions)
        } else {
            self.collect_object(event, changes, covered_deletions)
        }
    }
}
