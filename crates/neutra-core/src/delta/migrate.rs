use super::*;

impl DeltaIndex {
    pub(super) fn migrate_format(&mut self, path: &Path, generation: u64) -> io::Result<()> {
        // Keep the old WAL authoritative until the replacement is synced.
        let staged = crate::compact::suffix_path(path, ".migrate");
        let mut rewritten = open_reset_writer(&staged, generation)?;
        let mut bytes = 0u64;
        let mut since_flush = 0usize;
        {
            let mut append = |change: DeltaChange| -> io::Result<()> {
                bytes += append_frame(&mut rewritten, &change)?;
                since_flush += 1;
                if since_flush >= 4096 {
                    rewritten.flush()?;
                    since_flush = 0;
                }
                Ok(())
            };
            self.for_each_removed_prefix(|path| append(DeltaChange::RemovePrefix(path)))?;
            self.for_each_removed(|path| append(DeltaChange::Remove(path)))?;
            self.for_each_upsert(|record| append(DeltaChange::Upsert(record)))?;
        }
        rewritten.flush()?;
        rewritten.get_ref().sync_all()?;
        drop(rewritten);
        crate::compact_build::replace_file(&staged, path)?;
        crate::compact_build::sync_parent(path)?;
        let rewritten = open_append_private(path)?;
        #[cfg(unix)]
        {
            self.identity = file_identity_from_metadata(&rewritten.metadata()?)?;
        }
        let cursor = overlay_cursor(generation, HEADER + bytes, false, &rewritten.metadata()?)?;
        self.overlay
            .get_mut()
            .map_err(|_| io::Error::other("delta overlay lock poisoned"))?
            .set_cursor(&cursor)?;
        self.writer = Some(BufWriter::with_capacity(64 * 1024, rewritten));
        self.wal_bytes = HEADER + bytes;
        self.wal_v1 = false;
        Ok(())
    }
}
