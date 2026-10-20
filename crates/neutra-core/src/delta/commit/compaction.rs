use super::*;

#[derive(Clone, Copy)]
pub struct CompactionCursor {
    generation: u64,
    bytes: u64,
    identity: (u64, u64),
}

impl DeltaIndex {
    pub fn compaction_cursor(&self) -> CompactionCursor {
        CompactionCursor {
            generation: self.generation,
            bytes: self.wal_bytes,
            identity: self.identity,
        }
    }

    /// Stage only events received after the frozen view, retaining their order.
    /// The caller excludes event writes until this tail has been installed.
    pub fn stage_compaction_tail(
        &self,
        frozen: CompactionCursor,
        generation: u64,
        staged: &Path,
    ) -> io::Result<u64> {
        if self.writer.is_none()
            || self.generation != frozen.generation
            || self.identity != frozen.identity
            || self.wal_bytes < frozen.bytes
            || frozen.bytes < HEADER
            || generation == 0
            || generation == frozen.generation
            || self.wal_v1
        {
            return Err(invalid(
                "compaction cursor no longer matches the live writer",
            ));
        }
        let file = File::open(&self.path)?;
        let metadata = file.metadata()?;
        if file_identity_from_metadata(&metadata)? != frozen.identity
            || metadata.len() != self.wal_bytes
        {
            return Err(invalid("compaction WAL changed outside its writer"));
        }
        let mut reader = BufReader::new(file);
        read_header(&mut reader, frozen.generation)?;
        verify_frame_prefix(&mut reader, frozen.bytes)?;
        let mut output = crate::compact_build::open_private(staged)?;
        let result = (|| {
            output.write_all(MAGIC)?;
            output.write_all(&generation.to_le_bytes())?;
            let copied = copy_tail(&mut reader, &mut output, self.wal_bytes - frozen.bytes)?;
            output.flush()?;
            output.get_ref().sync_all()?;
            Ok(HEADER + copied)
        })();
        drop(output);
        if result.is_err() {
            let _ = std::fs::remove_file(staged);
        }
        result
    }

    /// Atomically replace the log without releasing its stable writer lease.
    /// A staged base must already be durable for marker-lost crash recovery.
    pub fn install_compaction_tail(
        &mut self,
        staged: &Path,
        generation: u64,
        bytes: u64,
    ) -> io::Result<()> {
        if self.writer.is_none()
            || generation == 0
            || generation == self.generation
            || bytes < HEADER
        {
            return Err(invalid("compaction publication requires a live writer"));
        }
        let file = File::open(staged)?;
        validate_private_file(&file)?;
        if file.metadata()?.len() != bytes {
            return Err(invalid("staged WAL length mismatch"));
        }
        let mut reader = BufReader::new(file);
        read_header(&mut reader, generation)?;
        verify_frame_prefix(&mut reader, bytes)?;
        drop(reader);
        self.sync()?;
        drop(self.writer.take());
        crate::compact_build::replace_file(staged, &self.path)?;
        crate::compact_build::sync_parent(&self.path)?;
        crate::compact_build::sync_parent(staged)?;
        let file = File::open(&self.path)?;
        let metadata = file.metadata()?;
        let overlay = self
            .overlay
            .get_mut()
            .map_err(|_| io::Error::other("delta overlay lock poisoned"))?;
        let restored = restore::restore(&file, overlay, generation, self.compact_at, true)?;
        if restored.bytes != bytes {
            return Err(invalid("published WAL contains an incomplete tail"));
        }
        self.identity = file_identity_from_metadata(&metadata)?;
        self.generation = generation;
        self.wal_bytes = bytes;
        self.wal_v1 = restored.legacy;
        self.writer = Some(BufWriter::with_capacity(
            64 * 1024,
            open_append_private(&self.path)?,
        ));
        Ok(())
    }
}

fn copy_tail(reader: &mut impl Read, writer: &mut impl Write, bytes: u64) -> io::Result<u64> {
    let mut copied = 0;
    while copied < bytes {
        if bytes - copied < 8 {
            return Err(invalid("compaction tail ends inside a frame header"));
        }
        let mut header = [0u8; 8];
        reader.read_exact(&mut header)?;
        let len = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize;
        let expected = u32::from_le_bytes(header[4..].try_into().unwrap());
        if len > MAX_FRAME || len as u64 > bytes - copied - 8 {
            return Err(invalid("compaction tail ends inside a frame"));
        }
        let mut payload = vec![0; len];
        reader.read_exact(&mut payload)?;
        if crc32fast::hash(&payload) != expected {
            return Err(invalid("compaction tail checksum mismatch"));
        }
        decode_change(false, &payload).map_err(codec)?;
        writer.write_all(&header)?;
        writer.write_all(&payload)?;
        copied += 8 + len as u64;
    }
    Ok(copied)
}
