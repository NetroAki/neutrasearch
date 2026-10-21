use super::*;

pub(super) fn read_sidecar(path: &Path) -> io::Result<(u64, Vec<DirectorySummaryEntry>)> {
    let bytes = read_sidecar_bytes(path)?;
    decode_sidecar(&bytes)
}

pub(super) fn read_sidecar_bytes(path: &Path) -> io::Result<Vec<u8>> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(invalid("directory summary sidecar is not a regular file"));
    }
    if metadata.len() > MAX_SIDECAR_BYTES {
        return Err(invalid("directory summary sidecar is too large"));
    }
    let mut file = File::open(path)?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

pub(super) fn sidecar_generation(bytes: &[u8]) -> io::Result<u64> {
    if bytes.len() < PREFIX_BYTES + TRAILER_BYTES || &bytes[..8] != MAGIC {
        return Err(invalid("not a Neutrasearch directory summary"));
    }
    let version = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    if !(MIN_READABLE_VERSION..=VERSION).contains(&version) {
        return Err(invalid("unsupported directory summary version"));
    }
    let generation = u64::from_le_bytes(bytes[12..20].try_into().unwrap());
    let trailer = bytes.len() - TRAILER_BYTES;
    let uncompressed = u64::from_le_bytes(bytes[trailer..trailer + 8].try_into().unwrap());
    if generation == 0 || uncompressed > MAX_UNCOMPRESSED_BYTES {
        return Err(invalid("invalid directory summary header"));
    }
    let expected_crc = u32::from_le_bytes(bytes[trailer + 8..].try_into().unwrap());
    if crc32fast::hash(&bytes[PREFIX_BYTES..trailer]) != expected_crc {
        return Err(invalid("directory summary checksum mismatch"));
    }
    Ok(generation)
}

/// Read one sidecar frame in the layout selected by the file version.
/// Version comes from the sidecar header, never from probing the frame:
/// old and new frames share a bincode prefix, so sniffing misdecodes.
pub(super) fn decode_entry(version: u32, frame: &[u8]) -> bincode::Result<DirectorySummaryEntry> {
    if version >= 2 {
        bincode::deserialize(frame)
    } else {
        Ok(DirectorySummaryEntry::from(bincode::deserialize::<
            OldSummaryEntry,
        >(frame)?))
    }
}

/// Frozen pre-physical entry layout, for sidecars written by older binaries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct OldSummaryEntry {
    pub(super) source: u32,
    pub(super) path: Box<str>,
    pub(super) logical_bytes: u64,
    pub(super) file_count: u64,
    pub(super) directory_count: u64,
    pub(super) children: Vec<OldSummaryChild>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct OldSummaryChild {
    pub(super) path: Box<str>,
    pub(super) kind: FileKind,
    pub(super) logical_bytes: u64,
    pub(super) file_count: u64,
    pub(super) directory_count: u64,
}

impl From<OldSummaryEntry> for DirectorySummaryEntry {
    fn from(old: OldSummaryEntry) -> Self {
        Self {
            source: old.source,
            path: old.path,
            logical_bytes: old.logical_bytes,
            physical_bytes: 0,
            file_count: old.file_count,
            directory_count: old.directory_count,
            children: old.children.into_iter().map(DirectoryChild::from).collect(),
        }
    }
}

impl From<OldSummaryChild> for DirectoryChild {
    fn from(old: OldSummaryChild) -> Self {
        Self {
            path: old.path,
            kind: old.kind,
            logical_bytes: old.logical_bytes,
            physical_bytes: 0,
            file_count: old.file_count,
            directory_count: old.directory_count,
        }
    }
}
/// Walk validated frames in file order, emitting each entry without
/// retaining the payload. Skips the entry sort and cross-entry validation
/// of decode_sidecar: consumers that need binary search or parent/child
/// consistency use decode_sidecar. Returns the file layout version and
/// base generation from the header.
pub(crate) fn walk_frames(
    bytes: &[u8],
    emit: &mut impl FnMut(DirectorySummaryEntry) -> io::Result<()>,
) -> io::Result<(u32, u64)> {
    if bytes.len() < PREFIX_BYTES + TRAILER_BYTES || &bytes[..8] != MAGIC {
        return Err(invalid("not a Neutrasearch directory summary"));
    }
    let version = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    if !(MIN_READABLE_VERSION..=VERSION).contains(&version) {
        return Err(invalid("unsupported directory summary version"));
    }
    let generation = u64::from_le_bytes(bytes[12..20].try_into().unwrap());
    let trailer = bytes.len() - TRAILER_BYTES;
    let uncompressed = u64::from_le_bytes(bytes[trailer..trailer + 8].try_into().unwrap());
    if generation == 0 || uncompressed > MAX_UNCOMPRESSED_BYTES {
        return Err(invalid("invalid directory summary header"));
    }
    let expected_crc = u32::from_le_bytes(bytes[trailer + 8..].try_into().unwrap());
    let compressed = &bytes[PREFIX_BYTES..trailer];
    if crc32fast::hash(compressed) != expected_crc {
        return Err(invalid("directory summary checksum mismatch"));
    }
    // Parse length-prefixed frames incrementally: buffering the whole
    // payload would pin ~20 GiB on a 100M-record host. Only complete
    // frames plus one read chunk are retained.
    let decoder = zstd::stream::Decoder::new(compressed).map_err(codec)?;
    walk_decoded_frames(decoder, version, emit)?;
    Ok((version, generation))
}

pub(crate) fn walk_file_frames(
    file: &mut File,
    emit: &mut impl FnMut(DirectorySummaryEntry) -> io::Result<()>,
) -> io::Result<(u32, u64)> {
    use std::io::{Seek, SeekFrom};
    let length = file.metadata()?.len();
    if length < (PREFIX_BYTES + TRAILER_BYTES) as u64 {
        return Err(invalid("truncated directory summary"));
    }
    let mut header = [0u8; PREFIX_BYTES];
    file.read_exact(&mut header)?;
    if &header[..8] != MAGIC {
        return Err(invalid("not a Neutrasearch directory summary"));
    }
    let version = u32::from_le_bytes(header[8..12].try_into().unwrap());
    let generation = u64::from_le_bytes(header[12..20].try_into().unwrap());
    if !(MIN_READABLE_VERSION..=VERSION).contains(&version) || generation == 0 {
        return Err(invalid("invalid directory summary header"));
    }
    let trailer = length - TRAILER_BYTES as u64;
    file.seek(SeekFrom::Start(trailer))?;
    let mut tail = [0u8; TRAILER_BYTES];
    file.read_exact(&mut tail)?;
    if u64::from_le_bytes(tail[..8].try_into().unwrap()) > MAX_UNCOMPRESSED_BYTES {
        return Err(invalid("directory summary payload exceeds safety cap"));
    }
    file.seek(SeekFrom::Start(PREFIX_BYTES as u64))?;
    let compressed_bytes = trailer - PREFIX_BYTES as u64;
    let mut hasher = crc32fast::Hasher::new();
    let mut buffer = [0u8; 65536];
    let mut limited = (&mut *file).take(compressed_bytes);
    loop {
        let n = limited.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    if hasher.finalize() != u32::from_le_bytes(tail[8..].try_into().unwrap()) {
        return Err(invalid("directory summary checksum mismatch"));
    }
    file.seek(SeekFrom::Start(PREFIX_BYTES as u64))?;
    let decoder = zstd::stream::Decoder::new(file.take(compressed_bytes)).map_err(codec)?;
    walk_decoded_frames(decoder, version, emit)?;
    Ok((version, generation))
}

pub(super) fn walk_decoded_frames(
    mut decoder: impl Read,
    version: u32,
    emit: &mut impl FnMut(DirectorySummaryEntry) -> io::Result<()>,
) -> io::Result<()> {
    let mut buf = Vec::new();
    let mut chunk = vec![0u8; 1024 * 1024];
    loop {
        let mut cursor = 0usize;
        loop {
            if buf.len() - cursor < 4 {
                break;
            }
            let length = u32::from_le_bytes(buf[cursor..cursor + 4].try_into().unwrap()) as usize;
            let frame_end = cursor
                .checked_add(4)
                .and_then(|start| start.checked_add(length))
                .ok_or_else(|| invalid("directory summary frame length overflow"))?;
            if frame_end > buf.len() {
                break;
            }
            emit(decode_entry(version, &buf[cursor + 4..frame_end]).map_err(codec)?)?;
            cursor = frame_end;
        }
        buf.drain(..cursor);
        let read = decoder.read(&mut chunk).map_err(codec)?;
        if read == 0 {
            if !buf.is_empty() {
                return Err(invalid("truncated directory summary frame"));
            }
            break;
        }
        buf.extend_from_slice(&chunk[..read]);
        if buf.len() as u64 > MAX_UNCOMPRESSED_BYTES + 1 {
            return Err(invalid("directory summary payload exceeds safety cap"));
        }
    }
    Ok(())
}

pub(super) fn decode_sidecar(bytes: &[u8]) -> io::Result<(u64, Vec<DirectorySummaryEntry>)> {
    let mut entries = Vec::new();
    let (_, generation) = walk_frames(bytes, &mut |entry| {
        entries.push(entry);
        Ok(())
    })?;
    validate_entries(&entries)?;
    entries.sort_unstable_by(|left, right| {
        compare_key(left.source, &left.path, right.source, &right.path)
    });
    Ok((generation, entries))
}

pub(super) fn validate_entries(entries: &[DirectorySummaryEntry]) -> io::Result<()> {
    let keys = entries
        .iter()
        .map(|entry| entry_key(entry.source, &entry.path))
        .collect::<HashSet<_>>();
    for entry in entries {
        if normalize_path(&entry.path)? != entry.path.as_ref() {
            return Err(invalid("directory summary contains an unnormalized path"));
        }
        for child in &entry.children {
            if normalize_path(&child.path)? != child.path.as_ref() {
                return Err(invalid(
                    "directory summary contains an unnormalized child path",
                ));
            }
            if parent_path(&child.path) != entry.path.as_ref() {
                return Err(invalid("directory summary child has the wrong parent"));
            }
            if child.kind == FileKind::Dir && !keys.contains(&entry_key(entry.source, &child.path))
            {
                return Err(invalid("directory summary child directory is missing"));
            }
        }
    }
    Ok(())
}
