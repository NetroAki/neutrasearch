//! Writer for a folded base: appends copied and re-encoded blocks, collects
//! descriptors and the postings of new blocks, then lays out the tables.

use super::sidecar::Feeds;
use super::*;
use crate::compact_build::{open_private, write_u16, write_u32, write_u64, BuildStats};
use crate::compact_merge::{sorted_pairs, write_postings_from_pairs};
use crate::compact_stream::encode_block;
use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

pub(super) struct Output {
    file: BufWriter<File>,
    pairs: BufWriter<File>,
    pairs_path: PathBuf,
    descs: Vec<BlockDesc>,
    old_to_new: Vec<u32>,
    buffer: Vec<FileRecord>,
    count: u64,
    sidecar: Feeds,
}

impl Output {
    pub(super) fn begin(
        temp: &Path,
        spill_dir: &Path,
        staged: &Path,
        generation: u64,
        base: &CompactIndex,
    ) -> io::Result<Self> {
        let mut file = open_private(temp)?;
        file.seek(SeekFrom::Start(HEADER))?;
        let pairs_path = spill_dir.join("pairs.psp");
        Ok(Self {
            file,
            pairs: BufWriter::with_capacity(
                1 << 20,
                crate::compact_spill::create_private_file(&pairs_path)?,
            ),
            pairs_path,
            descs: Vec::new(),
            old_to_new: vec![u32::MAX; base.blocks.count],
            buffer: Vec::with_capacity(BLOCK_RECORDS),
            count: 0,
            sidecar: Feeds::begin(staged, generation)?,
        })
    }

    /// Append one record in path order.
    pub(super) fn push(&mut self, record: FileRecord) -> io::Result<()> {
        self.sidecar.push(&record)?;
        self.buffer.push(record);
        self.count += 1;
        if self.buffer.len() == BLOCK_RECORDS {
            self.flush()?;
        }
        Ok(())
    }

    /// Encode the pending records as a (possibly short) block.
    pub(super) fn flush(&mut self) -> io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        let id = self.descs.len() as u32;
        encode_block(
            &self.buffer,
            id,
            &mut self.file,
            &mut self.descs,
            &mut self.pairs,
        )?;
        self.buffer.clear();
        Ok(())
    }

    /// Copy an untouched base block verbatim; its postings carry over.
    pub(super) fn copy(
        &mut self,
        base: &CompactIndex,
        id: u32,
        records: Vec<FileRecord>,
    ) -> io::Result<()> {
        self.flush()?;
        let desc = base
            .block(id as usize)
            .ok_or_else(|| invalid("path block ID out of range"))?;
        let bytes = checked(&base.map, desc.offset as usize, desc.len as usize)?;
        let offset = self.file.stream_position()?;
        self.file.write_all(bytes)?;
        self.old_to_new[id as usize] = self.descs.len() as u32;
        self.descs.push(BlockDesc { offset, ..desc });
        for record in records {
            self.sidecar.push(&record)?;
            self.count += 1;
        }
        Ok(())
    }

    pub(super) fn finish(mut self, base: &CompactIndex, generation: u64) -> io::Result<BuildStats> {
        self.flush()?;
        let desc_offset = self.file.stream_position()?;
        for desc in &self.descs {
            write_u64(&mut self.file, desc.offset)?;
            write_u32(&mut self.file, desc.len)?;
            write_u16(&mut self.file, desc.count)?;
            write_u16(&mut self.file, 0)?;
        }
        let postings_offset = self.file.stream_position()?;
        self.pairs.flush()?;
        let carried = super::postings::carried_pairs(base, &self.old_to_new);
        let added = sorted_pairs(&self.pairs_path)?;
        let merged = super::postings::Merged::new(carried, added);
        let dict = write_postings_from_pairs(merged, &mut self.file)?;
        let dict_offset = self.file.stream_position()?;
        for entry in &dict {
            write_u32(&mut self.file, entry.gram)?;
            write_u32(&mut self.file, entry.len)?;
            write_u64(&mut self.file, entry.offset)?;
        }
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(MAGIC)?;
        write_u32(&mut self.file, VERSION)?;
        write_u32(&mut self.file, BLOCK_RECORDS as u32)?;
        write_u64(&mut self.file, self.count)?;
        write_u32(&mut self.file, self.descs.len() as u32)?;
        write_u32(&mut self.file, dict.len() as u32)?;
        write_u64(&mut self.file, desc_offset)?;
        write_u64(&mut self.file, postings_offset)?;
        write_u64(&mut self.file, dict_offset)?;
        write_u64(&mut self.file, generation)?;
        self.file.flush()?;
        let mut file = self.file.into_inner().map_err(|error| error.into_error())?;
        file.sync_all()?;
        file.seek(SeekFrom::Start(0))?;
        let mut checksum = crc32fast::Hasher::new();
        let mut buffer = vec![0u8; 1 << 20];
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            checksum.update(&buffer[..read]);
        }
        file.write_all(&checksum.finalize().to_le_bytes())?;
        file.sync_all()?;
        self.sidecar.finish()?;
        Ok(BuildStats {
            generation,
            records: self.count,
            blocks: self.descs.len() as u32,
            trigrams: dict.len() as u32,
            bytes: 0,
            wall_ms: 0,
        })
    }
}
