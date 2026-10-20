use super::*;

/// Frozen pre-disk WAL frame, for logs written by older binaries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) enum OldDeltaChange {
    Upsert(OldDeltaRecord),
    Remove(Box<str>),
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct OldDeltaRecord {
    pub(super) path: Box<str>,
    pub(super) size: u64,
    pub(super) mtime: i64,
    pub(super) mode: u32,
    pub(super) kind: crate::FileKind,
    pub(super) fs: crate::mounts::FsKind,
    #[serde(default)]
    pub(super) native_id: u64,
    #[serde(default)]
    pub(super) native_parent: u64,
    pub(super) source: u32,
}
/// Decode one WAL frame in the layout selected by the log magic. Version
/// comes from the file header, never from probing the frame.
pub(super) fn decode_change(v1: bool, payload: &[u8]) -> Result<DeltaChange, bincode::Error> {
    if !v1 {
        return bincode::deserialize(payload);
    }
    match bincode::deserialize::<OldDeltaChange>(payload)? {
        OldDeltaChange::Remove(path) => Ok(DeltaChange::Remove(path)),
        OldDeltaChange::Upsert(old) => Ok(DeltaChange::Upsert(FileRecord {
            path: old.path,
            size: old.size,
            disk: 0,
            mtime: old.mtime,
            mode: old.mode,
            kind: old.kind,
            fs: old.fs,
            native_id: old.native_id,
            native_parent: old.native_parent,
            source: old.source,
        })),
    }
}

pub(super) fn read_header(reader: &mut impl Read, generation: u64) -> io::Result<bool> {
    let mut magic = [0u8; 8];
    reader.read_exact(&mut magic)?;
    let wal_v1 = if &magic == MAGIC || &magic == MAGIC_V3 || &magic == MAGIC_V2 {
        false
    } else if &magic == MAGIC_V1 {
        true
    } else {
        return Err(invalid("not a Neutrasearch delta log"));
    };
    let mut stored_generation = [0u8; 8];
    reader.read_exact(&mut stored_generation)?;
    if u64::from_le_bytes(stored_generation) != generation {
        return Err(invalid("delta log belongs to a different base generation"));
    }
    Ok(wal_v1)
}

pub(super) fn append_frame(writer: &mut impl Write, change: &DeltaChange) -> io::Result<u64> {
    let payload = bincode::serialize(change).map_err(codec)?;
    if payload.len() > MAX_FRAME {
        return Err(invalid("delta change exceeds safety cap"));
    }
    writer.write_all(&(payload.len() as u32).to_le_bytes())?;
    writer.write_all(&crc32fast::hash(&payload).to_le_bytes())?;
    writer.write_all(&payload)?;
    Ok(payload.len() as u64 + 8)
}

pub(super) fn verify_frame_prefix(reader: &mut (impl Read + Seek), end: u64) -> io::Result<()> {
    if end < HEADER {
        return Err(invalid("delta cache cursor precedes WAL header"));
    }
    reader.seek(SeekFrom::Start(HEADER))?;
    let mut verified = HEADER;
    let mut scratch = [0u8; 64 * 1024];
    while verified < end {
        if end - verified < 8 {
            return Err(invalid("delta cache cursor falls inside a frame header"));
        }
        let mut frame_header = [0u8; 8];
        reader.read_exact(&mut frame_header)?;
        let len = u32::from_le_bytes(frame_header[..4].try_into().expect("fixed slice")) as usize;
        let expected = u32::from_le_bytes(frame_header[4..].try_into().expect("fixed slice"));
        if len > MAX_FRAME || len as u64 > end - verified - 8 {
            return Err(invalid("delta cache cursor falls inside a frame"));
        }
        let mut remaining = len;
        let mut crc = crc32fast::Hasher::new();
        while remaining > 0 {
            let take = remaining.min(scratch.len());
            reader.read_exact(&mut scratch[..take])?;
            crc.update(&scratch[..take]);
            remaining -= take;
        }
        if crc.finalize() != expected {
            return Err(invalid("delta frame checksum mismatch"));
        }
        verified += 8 + len as u64;
    }
    if verified != end {
        return Err(invalid("delta cache cursor is not on a frame boundary"));
    }
    Ok(())
}

pub(super) fn replay_frames(
    reader: &mut impl Read,
    verified_bytes: &mut u64,
    overlay: &mut DeltaStore,
    wal_v1: bool,
    generation: u64,
    metadata: &Metadata,
) -> io::Result<()> {
    const REPLAY_BATCH: usize = 4096;
    let mut changes = Vec::with_capacity(REPLAY_BATCH);
    loop {
        let mut len = [0u8; 4];
        match reader.read_exact(&mut len) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(error),
        }
        let len = u32::from_le_bytes(len) as usize;
        if len > MAX_FRAME {
            return Err(invalid("delta frame exceeds safety cap"));
        }
        let mut expected_crc = [0u8; 4];
        match reader.read_exact(&mut expected_crc) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(error),
        }
        let mut payload = vec![0; len];
        match reader.read_exact(&mut payload) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(error),
        }
        let frame_end = *verified_bytes + 8 + len as u64;
        if crc32fast::hash(&payload) != u32::from_le_bytes(expected_crc) {
            return Err(invalid("delta frame checksum mismatch"));
        }
        let change: DeltaChange = decode_change(wal_v1, &payload).map_err(codec)?;
        changes.push(change);
        *verified_bytes = frame_end;
        if changes.len() == REPLAY_BATCH {
            let cursor = overlay_cursor(generation, *verified_bytes, wal_v1, metadata)?;
            overlay.apply_batch(&changes, &cursor)?;
            changes.clear();
        }
    }
    if !changes.is_empty() {
        let cursor = overlay_cursor(generation, *verified_bytes, wal_v1, metadata)?;
        overlay.apply_batch(&changes, &cursor)?;
    }
    Ok(())
}
