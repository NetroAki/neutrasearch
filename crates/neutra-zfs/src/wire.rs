use anyhow::{bail, Context, Result};
use neutra_core::{FileKind, FileRecord, FsKind, ScanStats};
use std::io::Read;
use std::path::{Component, Path};

pub(super) fn read(
    input: &mut impl Read,
    mountpoint: &Path,
    sink: &mut dyn FnMut(FileRecord),
) -> Result<ScanStats> {
    let root = mountpoint.to_str().context("ZFS mount path is not UTF-8")?;
    if !mountpoint.is_absolute() {
        bail!("ZFS mount path must be absolute");
    }
    let mut magic = [0; 8];
    input
        .read_exact(&mut magic)
        .context("ZFS metadata stream header")?;
    if &magic != b"NSZF0001" {
        bail!("Unsupported ZFS metadata stream version");
    }
    let mut stats = ScanStats::default();
    loop {
        let mut length = [0; 4];
        input
            .read_exact(&mut length)
            .context("Incomplete ZFS metadata stream")?;
        let length = u32::from_le_bytes(length) as usize;
        if length == 0 {
            if stats.records == 0 {
                bail!("ZFS metadata stream is missing its dataset root");
            }
            let mut extra = [0; 1];
            if input.read(&mut extra)? != 0 {
                bail!("Trailing bytes in ZFS metadata stream");
            }
            return Ok(stats);
        }
        if length > 4096 {
            bail!("ZFS metadata path exceeds the supported filesystem path length");
        }
        let mut fields = [0; 48];
        input.read_exact(&mut fields)?;
        let mut path = vec![0; length];
        input.read_exact(&mut path)?;
        // ponytail: byte-exact filenames need reversible native paths throughout the index and protocol.
        let relative = std::str::from_utf8(&path).context("ZFS filename is not UTF-8")?;
        validate_path(relative)?;
        let value =
            |field: usize| u64::from_le_bytes(fields[field * 8..field * 8 + 8].try_into().unwrap());
        let mode = u32::try_from(value(3)).context("Invalid ZFS file mode")?;
        let kind = match mode & 0o170000 {
            0o100000 => FileKind::File,
            0o040000 => FileKind::Dir,
            0o120000 => FileKind::Symlink,
            _ => FileKind::Other,
        };
        let path = if relative == "/" {
            root.to_owned()
        } else {
            format!("{}{relative}", root.trim_end_matches('/'))
        };
        if stats.records == 0 && (relative != "/" || kind != FileKind::Dir) {
            bail!("ZFS metadata stream is missing its dataset root");
        }
        if stats.records != 0 && relative == "/" {
            bail!("Duplicate dataset root in ZFS metadata stream");
        }
        sink(FileRecord {
            path: path.into_boxed_str(),
            size: value(0),
            disk: FileRecord::allocated_bytes(value(1)),
            mtime: i64::try_from(value(2)).context("Invalid ZFS modified time")?,
            mode,
            kind,
            fs: FsKind::Zfs,
            native_id: value(4),
            native_parent: value(5),
            source: 0,
        });
        stats.records += 1;
        stats.dirs += u64::from(kind == FileKind::Dir);
        stats.files += u64::from(kind != FileKind::Dir);
        stats.bytes_read += 52 + length as u64;
    }
}

fn validate_path(path: &str) -> Result<()> {
    if !path.starts_with('/') || path.contains('\0') || path.contains("//") {
        bail!("Invalid ZFS metadata path");
    }
    if Path::new(path)
        .components()
        .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
        || path.split('/').any(|part| part == "." || part == "..")
    {
        bail!("ZFS metadata path escapes its dataset");
    }
    Ok(())
}

#[cfg(test)]
mod tests;
