use super::*;

mod shared;

impl DeltaIndex {
    pub(super) fn open_mode(
        path: &Path,
        generation: u64,
        compact_at: u64,
        writable: bool,
    ) -> io::Result<Self> {
        if generation == 0 {
            return Err(invalid("delta requires a nonzero base generation"));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Lock a stable sibling rather than the WAL itself. Windows byte-range
        // locks are mandatory and would otherwise prevent read-only snapshots.
        let lock_file = if writable {
            let file = open_append_private(&lock_path(path))?;
            file.try_lock_exclusive().map_err(|error| {
                io::Error::new(
                    io::ErrorKind::WouldBlock,
                    format!("delta log already has a writer: {error}"),
                )
            })?;
            Some(file)
        } else {
            None
        };
        let wal_file = match File::open(path) {
            Ok(file) => Some(file),
            Err(error) if writable && error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        let metadata = wal_file.as_ref().map(File::metadata).transpose()?;
        let file_bytes = metadata.as_ref().map_or(0, Metadata::len);
        let mut overlay = if writable {
            DeltaStore::open(path)?
        } else {
            DeltaStore::open_private(path)?
        };
        if file_bytes == 0 {
            overlay.clear_unknown()?;
        }
        let mut next_compact_at = compact_at.max(HEADER + 1);
        let mut wal_bytes = file_bytes;
        let mut wal_v1 = false;
        #[cfg(unix)]
        let mut identity = None;
        if let Some(file) = wal_file.as_ref().filter(|_| file_bytes > 0) {
            let restored = restore::restore(file, &mut overlay, generation, compact_at, writable)?;
            wal_bytes = restored.bytes;
            wal_v1 = restored.legacy;
            next_compact_at = restored.compact_at;
            #[cfg(unix)]
            {
                identity = restored.identity;
            }
        }
        // Publish the new WAL format before accepting prefix frames.
        let migrated = writable && file_bytes > 0 && {
            let mut magic = [0u8; 8];
            File::open(path)?.read_exact(&mut magic)?;
            &magic != MAGIC
        };
        let writer = if writable {
            if migrated {
                // Keep the verified legacy WAL intact until the complete new
                // representation has been synced and atomically published.
                None
            } else {
                if file_bytes > wal_bytes {
                    truncate_private(path, wal_bytes)?;
                }
                let mut file = open_append_private(path)?;
                if wal_bytes == 0 {
                    file.write_all(MAGIC)?;
                    file.write_all(&generation.to_le_bytes())?;
                    file.sync_data()?;
                    wal_bytes = HEADER;
                }
                Some(BufWriter::with_capacity(64 * 1024, file))
            }
        } else {
            None
        };
        let mut this = Self {
            path: path.to_path_buf(),
            generation,
            writer,
            lock_file,
            overlay: std::sync::Mutex::new(overlay),
            wal_bytes,
            compact_at: next_compact_at,
            wal_v1,
            #[cfg(unix)]
            identity: match identity {
                Some(identity) => identity,
                None => file_identity(path)?,
            },
        };
        if migrated {
            // Release the legacy WAL's read handle before publishing its replacement.
            drop(wal_file);
            this.migrate_format(path, generation)?;
        }
        Ok(this)
    }
}
