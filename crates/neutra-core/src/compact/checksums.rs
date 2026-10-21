use super::{checked, invalid, u32_at, u64_at, DESC_SIZE, HEADER};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};

fn ranges(header: &[u8], length: usize) -> io::Result<[(usize, usize); 3]> {
    let count = u32_at(header, 24)? as usize;
    let desc =
        usize::try_from(u64_at(header, 32)?).map_err(|_| invalid("descriptor offset overflow"))?;
    let bytes = count
        .checked_mul(DESC_SIZE as usize)
        .ok_or_else(|| invalid("descriptor length overflow"))?;
    let postings =
        usize::try_from(u64_at(header, 40)?).map_err(|_| invalid("postings offset overflow"))?;
    if postings > length {
        return Err(invalid("postings outside compact index"));
    }
    Ok([
        (0, HEADER as usize),
        (desc, bytes),
        (postings, length - postings),
    ])
}

pub(super) fn verify_routing(data: &[u8], expected: u32) -> io::Result<()> {
    let mut crc = crc32fast::Hasher::new();
    for (offset, length) in ranges(data, data.len())? {
        crc.update(checked(data, offset, length)?);
    }
    if crc.finalize() != expected {
        return Err(invalid("compact routing checksum mismatch"));
    }
    Ok(())
}

pub(crate) fn finish(file: &mut File) -> io::Result<()> {
    let length =
        usize::try_from(file.metadata()?.len()).map_err(|_| invalid("compact file too large"))?;
    file.seek(SeekFrom::Start(0))?;
    let mut header = [0; HEADER as usize];
    file.read_exact(&mut header)?;
    let mut routing = crc32fast::Hasher::new();
    let mut buffer = [0; 64 * 1024];
    for (offset, length) in ranges(&header, length)? {
        file.seek(SeekFrom::Start(offset as u64))?;
        hash_range(file, length, &mut routing, &mut buffer)?;
    }
    file.seek(SeekFrom::End(0))?;
    file.write_all(&routing.finalize().to_le_bytes())?;
    file.seek(SeekFrom::Start(0))?;
    let mut whole = crc32fast::Hasher::new();
    hash_range(file, length + 4, &mut whole, &mut buffer)?;
    file.write_all(&whole.finalize().to_le_bytes())?;
    file.sync_all()
}

fn hash_range(
    file: &mut File,
    mut length: usize,
    crc: &mut crc32fast::Hasher,
    buffer: &mut [u8],
) -> io::Result<()> {
    while length > 0 {
        let count = length.min(buffer.len());
        file.read_exact(&mut buffer[..count])?;
        crc.update(&buffer[..count]);
        length -= count;
    }
    Ok(())
}
