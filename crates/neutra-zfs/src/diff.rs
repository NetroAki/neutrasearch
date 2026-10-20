use anyhow::{bail, Context, Result};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffEntry {
    Created(PathBuf),
    Removed(PathBuf),
    Modified(PathBuf),
    Renamed { from: PathBuf, to: PathBuf },
}

/// Parse `zfs diff -H` output, with optional `-F` type columns.
pub fn parse_diff(input: &str) -> Result<Vec<DiffEntry>> {
    input
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(line_no, line)| {
            parse_line(line).with_context(|| format!("zfs diff line {}", line_no + 1))
        })
        .collect()
}

fn parse_line(line: &str) -> Result<DiffEntry> {
    let cols: Vec<_> = line.split('\t').collect();
    let marker = cols[0];
    let path_column = match cols.get(1) {
        Some(column) if cols.len() >= 3 && column.len() == 1 => 2,
        Some(column) if column.starts_with('/') => 1,
        _ => bail!("missing path or file type"),
    };
    let path_at = |column| -> Result<PathBuf> {
        decode_path(cols.get(column).copied().context("missing path")?)
    };
    match marker {
        "+" => Ok(DiffEntry::Created(path_at(path_column)?)),
        "-" => Ok(DiffEntry::Removed(path_at(path_column)?)),
        "M" => Ok(DiffEntry::Modified(path_at(path_column)?)),
        "R" => Ok(DiffEntry::Renamed {
            from: path_at(path_column)?,
            to: path_at(path_column + 1)?,
        }),
        other => bail!("unknown zfs diff marker {other:?}"),
    }
}

fn decode_path(encoded: &str) -> Result<PathBuf> {
    let mut bytes = Vec::with_capacity(encoded.len());
    let mut input = encoded.bytes();
    while let Some(byte) = input.next() {
        let value = if byte == b'\\' {
            let mut value = 0u16;
            // OpenZFS escapes bytes as four octal digits, including the leading zero.
            for _ in 0..4 {
                let digit = input.next().context("incomplete path escape")?;
                if !(b'0'..=b'7').contains(&digit) {
                    bail!("invalid octal path escape");
                }
                value = value * 8 + u16::from(digit - b'0');
            }
            u8::try_from(value).context("path escape exceeds a byte")?
        } else {
            byte
        };
        if value == 0 {
            bail!("NUL in ZFS path");
        }
        bytes.push(value);
    }
    if bytes.first() != Some(&b'/') {
        bail!("ZFS diff path is not absolute");
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(std::ffi::OsString::from_vec(bytes).into())
    }
    #[cfg(not(unix))]
    {
        Ok(String::from_utf8(bytes)
            .context("non-UTF-8 ZFS path")?
            .into())
    }
}
