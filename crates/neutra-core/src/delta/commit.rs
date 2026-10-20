use super::*;

impl DeltaIndex {
    pub fn apply(&mut self, change: DeltaChange) -> io::Result<()> {
        self.apply_batch(std::iter::once(change))?;
        Ok(())
    }

    /// Append a batch of changes with one writer flush at the end. Event
    /// watchers deliver bursts; flushing per frame would cost one write
    /// syscall per change.
    pub fn apply_batch<I: IntoIterator<Item = DeltaChange>>(
        &mut self,
        changes: I,
    ) -> io::Result<u32> {
        self.overlay
            .get_mut()
            .map_err(|_| io::Error::other("delta overlay lock poisoned"))?
            .ensure_live()?;
        if self.writer.is_none() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "read-only delta snapshot",
            ));
        }
        let mut pending = Vec::new();
        let mut pending_bytes = self.wal_bytes;
        let mut count = 0u32;
        for change in changes {
            if let DeltaChange::RemovePrefix(path) = &change {
                if !crate::query::safe_absolute_path(path)
                    || crate::paths::portable_root_prefix(path) != **path
                {
                    return Err(invalid(
                        "prefix removal requires an absolute normalized path",
                    ));
                }
            }
            let payload_size = bincode::serialized_size(&change).map_err(codec)? as usize;
            if payload_size > MAX_FRAME {
                return Err(invalid("delta change exceeds safety cap"));
            }
            if pending.len() == 4096 {
                self.flush_overlay_batch(&pending, pending_bytes)?;
                pending.clear();
                self.wal_bytes = pending_bytes;
            }
            let writer = self.writer.as_mut().expect("writer checked");
            pending_bytes = pending_bytes.saturating_add(append_frame(writer, &change)?);
            pending.push(change);
            count = count.saturating_add(1);
        }
        self.writer.as_mut().ok_or_else(|| {
            io::Error::new(io::ErrorKind::PermissionDenied, "read-only delta snapshot")
        })?;
        if !pending.is_empty() {
            self.flush_overlay_batch(&pending, pending_bytes)?;
            self.wal_bytes = pending_bytes;
        }
        Ok(count)
    }

    fn flush_overlay_batch(&mut self, pending: &[DeltaChange], end_bytes: u64) -> io::Result<()> {
        let writer = self.writer.as_mut().ok_or_else(|| {
            io::Error::new(io::ErrorKind::PermissionDenied, "read-only delta snapshot")
        })?;
        writer.flush()?;
        writer.get_ref().sync_data()?;
        let metadata = writer.get_ref().metadata()?;
        let cursor = overlay_cursor(self.generation, end_bytes, self.wal_v1, &metadata)?;
        let overlay = self
            .overlay
            .get_mut()
            .map_err(|_| io::Error::other("delta overlay lock poisoned"))?;
        if let Err(error) = overlay.apply_batch(pending, &cursor) {
            overlay.mark_stale();
            return Err(error);
        }
        Ok(())
    }
    pub fn sync(&mut self) -> io::Result<()> {
        let writer = self.writer.as_mut().ok_or_else(|| {
            io::Error::new(io::ErrorKind::PermissionDenied, "read-only delta snapshot")
        })?;
        writer.flush()?;
        writer.get_ref().sync_data()
    }

    /// Collapse repeated changes without decoding or rebuilding the immutable
    /// base. The stable writer lock covers the atomic log replacement.
    #[cfg(unix)]
    pub fn checkpoint(&mut self) -> io::Result<()> {
        self.sync()?;
        let staged = crate::compact::suffix_path(&self.path, ".checkpoint");
        match std::fs::remove_file(&staged) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let mut writer = open_reset_writer(&staged, self.generation)?;
        let mut bytes = HEADER;
        self.for_each_removed_prefix(|path| {
            bytes += append_frame(&mut writer, &DeltaChange::RemovePrefix(path))?;
            Ok(())
        })?;
        self.for_each_removed(|path| {
            bytes += append_frame(&mut writer, &DeltaChange::Remove(path))?;
            Ok(())
        })?;
        self.for_each_upsert(|record| {
            bytes += append_frame(&mut writer, &DeltaChange::Upsert(record))?;
            Ok(())
        })?;
        writer.flush()?;
        writer.get_ref().sync_all()?;
        drop(writer);
        crate::compact_build::replace_file(&staged, &self.path)?;
        self.writer = Some(BufWriter::with_capacity(
            64 * 1024,
            open_append_private(&self.path)?,
        ));
        let wal_file = File::open(&self.path)?;
        let wal_metadata = wal_file.metadata()?;
        self.identity = file_identity_from_metadata(&wal_metadata)?;
        self.wal_bytes = bytes;
        self.overlay
            .get_mut()
            .map_err(|_| io::Error::other("delta overlay lock poisoned"))?
            .set_checkpoint_cursor(&overlay_cursor(
                self.generation,
                self.wal_bytes,
                self.wal_v1,
                &wal_metadata,
            )?)?;
        crate::compact_build::sync_parent(&self.path)
    }
    /// Reset this writer to an empty WAL for a replacement base generation.
    /// The stable writer lock remains held throughout the transition.
    pub fn reset(&mut self, generation: u64) -> io::Result<()> {
        if generation == 0 {
            return Err(invalid("delta requires a nonzero base generation"));
        }
        let mut old_writer = self.writer.take().ok_or_else(|| {
            io::Error::new(io::ErrorKind::PermissionDenied, "read-only delta snapshot")
        })?;
        old_writer.flush()?;
        old_writer.get_ref().sync_data()?;
        drop(old_writer);
        // Windows cannot truncate through an append-mode handle. Keep the
        // sibling writer lock, close append, truncate with a dedicated handle,
        // then reopen append for subsequent frames.
        let writer = open_reset_writer(&self.path, generation)?;
        let wal_metadata = writer.get_ref().metadata()?;
        self.writer = Some(writer);
        self.generation = generation;
        self.wal_bytes = HEADER;
        self.overlay
            .get_mut()
            .map_err(|_| io::Error::other("delta overlay lock poisoned"))?
            .clear(&overlay_cursor(generation, HEADER, false, &wal_metadata)?)?;
        Ok(())
    }
}

impl Drop for DeltaIndex {
    fn drop(&mut self) {
        // Explicitly release BSD flock locks before closing the descriptor.
        // macOS can otherwise briefly report the just-dropped writer as busy
        // when the same process immediately reopens it.
        if let Some(lock_file) = self.lock_file.take() {
            let _ = FileExt::unlock(&lock_file);
        }
    }
}

#[cfg(target_os = "linux")]
mod compaction;
#[cfg(target_os = "linux")]
pub use compaction::CompactionCursor;
