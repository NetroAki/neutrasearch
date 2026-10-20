use super::*;

impl DeltaIndex {
    /// A query-scoped read transaction on the writer's cache. Drop it after
    /// the query so idle readers cannot retain the SQLite write-ahead log.
    pub fn open_search_snapshot(path: &Path, generation: u64) -> io::Result<Self> {
        for attempt in 0..4 {
            match Self::search_snapshot_once(path, generation) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock && attempt < 3 => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                result => return result,
            }
        }
        unreachable!("last snapshot attempt returns its result")
    }

    fn search_snapshot_once(path: &Path, generation: u64) -> io::Result<Self> {
        let mut file = File::open(path)?;
        let initial_bytes = file.metadata()?.len();
        let legacy = read_header(&mut file, generation)?;
        let overlay = DeltaStore::open_shared(path)?;
        let cursor = overlay
            .cursor()?
            .ok_or_else(|| invalid("live cache has no cursor"))?;
        let metadata = file.metadata()?;
        let current = overlay_cursor(generation, cursor.wal_bytes, legacy, &metadata)?;
        if cursor.generation != generation
            || cursor.legacy_frames != legacy
            || !same_file_identity(cursor, current)
            || cursor.wal_bytes < initial_bytes
            || cursor.wal_bytes > metadata.len()
            || (cursor.wal_bytes == metadata.len() && cursor.modified_ns != current.modified_ns)
        {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "live cache is catching up with the durable log",
            ));
        }
        verify_frame_prefix(&mut BufReader::new(file), cursor.wal_bytes)?;
        Ok(Self {
            path: path.to_path_buf(),
            generation,
            writer: None,
            lock_file: None,
            overlay: std::sync::Mutex::new(overlay),
            wal_bytes: cursor.wal_bytes,
            compact_at: DEFAULT_COMPACT_AT,
            wal_v1: legacy,
            #[cfg(unix)]
            identity: file_identity_from_metadata(&metadata)?,
        })
    }
}
