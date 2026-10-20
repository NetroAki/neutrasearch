//! Positional buffered writers for the fixed-size column sections.
use std::{fs::File, io};

const FLUSH: usize = 1 << 20;

pub(super) fn write_all_at(file: &File, bytes: &[u8], offset: u64) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::FileExt::write_all_at(file, bytes, offset)
    }
    #[cfg(windows)]
    {
        let (mut done, mut offset) = (0, offset);
        while done < bytes.len() {
            let n = std::os::windows::fs::FileExt::seek_write(file, &bytes[done..], offset)?;
            if n == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            done += n;
            offset += n as u64;
        }
        Ok(())
    }
}

pub(super) fn read_exact_at(file: &File, bytes: &mut [u8], offset: u64) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::FileExt::read_exact_at(file, bytes, offset)
    }
    #[cfg(windows)]
    {
        let (mut done, mut offset) = (0, offset);
        while done < bytes.len() {
            let n = std::os::windows::fs::FileExt::seek_read(file, &mut bytes[done..], offset)?;
            if n == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            done += n;
            offset += n as u64;
        }
        Ok(())
    }
}

pub(super) struct Section<'a> {
    file: &'a File,
    position: u64,
    buffer: Vec<u8>,
}

impl<'a> Section<'a> {
    pub(super) fn new(file: &'a File, start: u64) -> Self {
        Self {
            file,
            position: start,
            buffer: Vec::with_capacity(FLUSH + 64),
        }
    }

    pub(super) fn push(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.buffer.extend_from_slice(bytes);
        if self.buffer.len() >= FLUSH {
            self.flush()?;
        }
        Ok(())
    }

    pub(super) fn flush(&mut self) -> io::Result<()> {
        write_all_at(self.file, &self.buffer, self.position)?;
        self.position += self.buffer.len() as u64;
        self.buffer.clear();
        Ok(())
    }

    pub(super) fn finish(mut self) -> io::Result<u64> {
        self.flush()?;
        Ok(self.position)
    }
}
