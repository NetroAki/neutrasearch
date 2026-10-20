use super::*;

pub(super) struct Restored {
    pub(super) bytes: u64,
    pub(super) legacy: bool,
    pub(super) compact_at: u64,
    #[cfg(unix)]
    pub(super) identity: Option<(u64, u64)>,
}

pub(super) fn restore(
    wal_file: &File,
    overlay: &mut DeltaStore,
    generation: u64,
    compact_at: u64,
    writable: bool,
) -> io::Result<Restored> {
    let file_bytes = wal_file.metadata()?.len();
    let next_compact_at = compact_at.max(HEADER + 1);
    #[cfg(unix)]
    let identity;
    let file = wal_file.try_clone()?;
    let metadata = file.metadata()?;
    #[cfg(unix)]
    {
        identity = Some(file_identity_from_metadata(&metadata)?);
    }
    let mut reader = BufReader::new(file.take(file_bytes));
    let wal_v1 = read_header(&mut reader, generation)?;
    let current = overlay_cursor(generation, HEADER, wal_v1, &metadata)?;
    let cached = overlay.cursor()?;
    let valid = cached.is_some_and(|c| {
        c.generation == generation
            && c.wal_bytes >= HEADER
            && c.wal_bytes <= file_bytes
            && c.legacy_frames == wal_v1
            && same_file_identity(c, current)
            && (c.wal_bytes < file_bytes || c.modified_ns == current.modified_ns)
    });
    let mut verified_bytes = HEADER;
    if valid {
        verified_bytes = cached.expect("validated cursor").wal_bytes;
        // The persistent overlay avoids SQL replay, but it never
        // substitutes for validating the WAL's authoritative bytes.
        // Stream frame CRCs without decoding payloads or allocating
        // per-record state before trusting a cached prefix.
        verify_frame_prefix(&mut reader, verified_bytes)?;
        reader.seek(SeekFrom::Start(verified_bytes))?;
        replay_frames(
            &mut reader,
            &mut verified_bytes,
            &mut *overlay,
            wal_v1,
            generation,
            &metadata,
        )?;
    } else {
        overlay.clear(&current)?;
        reader.seek(SeekFrom::Start(HEADER))?;
        replay_frames(
            &mut reader,
            &mut verified_bytes,
            &mut *overlay,
            wal_v1,
            generation,
            &metadata,
        )?;
    }
    let wal_bytes = verified_bytes;
    if writable {
        overlay.set_cursor(&overlay_cursor(
            generation,
            verified_bytes,
            wal_v1,
            &metadata,
        )?)?;
    }
    Ok(Restored {
        bytes: wal_bytes,
        legacy: wal_v1,
        compact_at: next_compact_at,
        #[cfg(unix)]
        identity,
    })
}
