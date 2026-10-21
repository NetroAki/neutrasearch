use super::*;
#[cfg(not(windows))]
use memmap2::Mmap;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

impl CompactIndex {
    /// Read just enough of the on-disk header to detect atomic base replacement.
    /// A caller that observes a change must reopen normally, which performs the
    /// complete structural and whole-file checksum validation.
    pub fn generation_on_disk(path: &Path) -> io::Result<u64> {
        let stale = suffix_path(path, ".stale");
        if stale.is_file() {
            return Err(invalid(format!(
                "compact index is marked stale by {} and requires a full rebuild",
                stale.display()
            )));
        }
        let mut file = File::open(path)?;
        let mut header = [0u8; HEADER as usize];
        file.read_exact(&mut header)?;
        if &header[..8] != MAGIC {
            return Err(invalid("invalid compact index header"));
        }
        let record_version = u32_at(&header, 8)?;
        if !(MIN_READABLE_VERSION..=VERSION).contains(&record_version) {
            return Err(invalid("invalid compact index header"));
        }
        u64_at(&header, 56)
    }

    pub fn open(path: &Path) -> io::Result<Self> {
        Self::open_impl(path, true)
    }

    /// Open without verifying the whole-payload checksum.
    ///
    /// Startup path for readers (GUI, MCP, query CLI): opening a multi-gigabyte
    /// index would otherwise hash the entire file before the first search.
    /// Version 8 validates routing metadata at open and block checksums on decode.
    /// Older bases require whole-file validation.
    pub fn open_fast(path: &Path) -> io::Result<Self> {
        Self::open_impl(path, false)
    }

    fn open_impl(path: &Path, verify_payload: bool) -> io::Result<Self> {
        let stale = suffix_path(path, ".stale");
        if stale.is_file() {
            return Err(invalid(format!(
                "compact index is marked stale by {} and requires a full rebuild",
                stale.display()
            )));
        }
        #[cfg(not(windows))]
        let mut file = File::open(path)?;
        #[cfg(not(windows))]
        let map = unsafe { Mmap::map(&file)? };
        #[cfg(windows)]
        let map = std::fs::read(path)?;
        if map.len() < HEADER as usize + CHECKSUM_BYTES || &map[..8] != MAGIC {
            return Err(invalid("not a Neutrasearch compact index"));
        }
        let data_len = map.len() - CHECKSUM_BYTES;
        let data = &map[..data_len];
        let record_version = u32_at(data, 8)?;
        if !(MIN_READABLE_VERSION..=VERSION).contains(&record_version) {
            return Err(invalid("unsupported compact index version"));
        }
        if verify_payload || record_version < 8 {
            let expected_checksum = u32::from_le_bytes(
                map[data_len..]
                    .try_into()
                    .map_err(|_| invalid("missing compact index checksum"))?,
            );
            #[cfg(not(windows))]
            let checksum = {
                use std::io::{Seek, SeekFrom};
                file.seek(SeekFrom::Start(0))?;
                let mut hasher = crc32fast::Hasher::new();
                let mut remaining = data_len;
                let mut buffer = [0u8; 64 * 1024];
                while remaining > 0 {
                    let take = remaining.min(buffer.len());
                    file.read_exact(&mut buffer[..take])?;
                    hasher.update(&buffer[..take]);
                    remaining -= take;
                }
                hasher.finalize()
            };
            #[cfg(windows)]
            let checksum = crc32fast::hash(data);
            if checksum != expected_checksum {
                return Err(invalid("compact index checksum mismatch"));
            }
        }
        if record_version >= 8 {
            let routing_end = data
                .len()
                .checked_sub(4)
                .ok_or_else(|| invalid("missing routing checksum"))?;
            let expected = u32::from_le_bytes(data[routing_end..].try_into().unwrap());
            super::checksums::verify_routing(&data[..routing_end], expected)?;
        }
        if u32_at(data, 12)? != BLOCK_RECORDS as u32 {
            return Err(invalid("unsupported path block size"));
        }
        let record_count = u64_at(data, 16)?;
        let generation = u64_at(data, 56)?;
        let block_count = u32_at(data, 24)? as usize;
        let dict_count = u32_at(data, 28)? as usize;
        let desc_offset = u64_at(data, 32)? as usize;
        let dict_offset = u64_at(data, 48)? as usize;
        for i in 0..block_count {
            let p = desc_offset.saturating_add(i.saturating_mul(DESC_SIZE as usize));
            let d = BlockDesc {
                offset: u64_at(data, p)?,
                len: u32_at(data, p + 8)?,
                count: u16_at(data, p + 12)?,
            };
            checked(data, d.offset as usize, d.len as usize)?;
        }
        let mut previous_gram = None;
        for i in 0..dict_count {
            let p = dict_offset.saturating_add(i.saturating_mul(DICT_SIZE as usize));
            let d = DictEntry {
                gram: u32_at(data, p)?,
                len: u32_at(data, p + 4)?,
                offset: u64_at(data, p + 8)?,
            };
            checked(data, d.offset as usize, d.len as usize)?;
            if previous_gram.is_some_and(|before| before >= d.gram) {
                return Err(invalid("trigram dictionary is not sorted"));
            }
            previous_gram = Some(d.gram);
        }
        #[cfg(unix)]
        unsafe {
            libc::madvise(
                map.as_ptr() as *mut libc::c_void,
                map.len(),
                libc::MADV_DONTNEED,
            );
        }
        Ok(Self {
            map,
            generation,
            record_count,
            blocks: TableView {
                start: desc_offset,
                count: block_count,
            },
            dict: TableView {
                start: dict_offset,
                count: dict_count,
            },
            record_version,
            starts: std::sync::OnceLock::new(),
        })
    }

    /// Open a coherent read-only base+delta pair, including either side of an
    /// in-progress journaled compaction. Readers prefer the current pair and
    /// use the staged base only when its generation matches the reset WAL.
    pub fn open_with_delta_snapshot(path: &Path) -> io::Result<(Self, Option<DeltaIndex>)> {
        Self::open_with_delta_snapshot_impl(path, true)
    }

    /// `open_with_delta_snapshot` with the startup fast path: the base is
    /// opened without whole-payload checksum verification (see `open_fast`).
    pub fn open_with_delta_snapshot_fast(path: &Path) -> io::Result<(Self, Option<DeltaIndex>)> {
        Self::open_with_delta_snapshot_impl(path, false)
    }

    fn open_with_delta_snapshot_impl(
        path: &Path,
        verify_payload: bool,
    ) -> io::Result<(Self, Option<DeltaIndex>)> {
        let base = Self::open_impl(path, verify_payload)?;
        let mut delta_path = path.to_path_buf();
        delta_path.set_extension("delta");
        if !delta_path.is_file() {
            return Ok((base, None));
        }
        match DeltaIndex::open_snapshot(&delta_path, base.generation()) {
            Ok(delta) => Ok((base, Some(delta))),
            Err(current_error) => {
                let marker = suffix_path(path, ".compacting");
                let staged_path = suffix_path(path, ".compact");
                if !marker.is_file() || !staged_path.is_file() {
                    return Err(current_error);
                }
                let staged = Self::open_impl(&staged_path, verify_payload)?;
                match DeltaIndex::open_snapshot(&delta_path, staged.generation()) {
                    Ok(delta) => Ok((staged, Some(delta))),
                    Err(staged_error) => Err(invalid(format!(
                        "neither current nor staged base matches the delta: current={current_error}; staged={staged_error}"
                    ))),
                }
            }
        }
    }
}
