use super::*;

#[cfg(test)]
pub(super) fn write_sidecar_from_order(
    records: &[FileRecord],
    order: &[u32],
    index_path: &Path,
    generation: u64,
) -> io::Result<()> {
    if generation == 0 {
        return Err(invalid("directory summary requires a nonzero generation"));
    }
    let destination = directory_summary_path(index_path);
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = temporary_path(&destination);
    let mut file = open_private_file(&temporary)?;
    file.write_all(MAGIC)?;
    file.write_all(&VERSION.to_le_bytes())?;
    file.write_all(&generation.to_le_bytes())?;
    let mut compressed = HashingWriter {
        inner: BufWriter::new(file),
        hasher: crc32fast::Hasher::new(),
    };
    let mut uncompressed_bytes = 0u64;
    {
        let mut encoder = zstd::stream::Encoder::new(&mut compressed, 3).map_err(codec)?;
        emit_records(records, order, |entry| {
            let encoded = bincode::serialize(&entry).map_err(codec)?;
            let len = u32::try_from(encoded.len())
                .map_err(|_| invalid("directory summary entry is too large"))?;
            encoder.write_all(&len.to_le_bytes())?;
            encoder.write_all(&encoded)?;
            uncompressed_bytes = uncompressed_bytes
                .checked_add(4 + encoded.len() as u64)
                .ok_or_else(|| invalid("directory summary payload is too large"))?;
            Ok(())
        })?;
        encoder.finish().map_err(codec)?;
    }
    let checksum = compressed.hasher.finalize();
    let mut file = compressed
        .inner
        .into_inner()
        .map_err(|error| error.into_error())?;
    file.write_all(&uncompressed_bytes.to_le_bytes())?;
    file.write_all(&checksum.to_le_bytes())?;
    file.sync_all()?;
    drop(file);
    replace_file(&temporary, &destination)?;
    sync_parent(&destination)
}
