//! Generation-bound native filesystem cursors captured before an initial scan.

use bincode::Options;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 8] = b"NTRACUR1";
const MAX_SEED_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeCursor {
    pub mount: String,
    pub tree: u64,
    pub since: u64,
}

pub(crate) fn seed_path(base: &Path) -> PathBuf {
    let mut name = base.as_os_str().to_os_string();
    name.push(".ingest-cursor");
    PathBuf::from(name)
}

fn private_temp(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".tmp-{}", std::process::id()));
    PathBuf::from(name)
}

fn open_temp(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    options.open(path)
}

fn open_seed(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    options.open(path)
}

/// Atomically save cursors bound to the compact base generation being published.
pub fn save_seed(base: &Path, generation: u64, cursors: &[NativeCursor]) -> io::Result<()> {
    validate_cursors(generation, cursors)?;
    let bytes = bincode::serialize(&(generation, cursors))
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let mut contents = Vec::with_capacity(MAGIC.len() + 8 + bytes.len());
    contents.extend_from_slice(MAGIC);
    contents.extend_from_slice(&crc32fast::hash(&bytes).to_le_bytes());
    contents.extend_from_slice(&bytes);
    let path = seed_path(base);
    let temporary = private_temp(&path);
    let _ = fs::remove_file(&temporary);
    let mut file = open_temp(&temporary)?;
    let result = (|| {
        file.write_all(&contents)?;
        file.sync_all()?;
        crate::compact_build::replace_file(&temporary, &path)?;
        crate::compact_build::sync_parent(&path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Load a cursor seed only when it matches the currently published base.
pub fn load_seed(base: &Path, generation: u64) -> io::Result<Vec<NativeCursor>> {
    let file = open_seed(&seed_path(base))?;
    if file.metadata()?.len() > MAX_SEED_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "native cursor seed exceeds size limit",
        ));
    }
    let mut contents = Vec::new();
    file.take(MAX_SEED_BYTES + 1).read_to_end(&mut contents)?;
    if contents.len() as u64 > MAX_SEED_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "native cursor seed exceeds size limit",
        ));
    }
    if contents.len() < MAGIC.len() + 4 || &contents[..MAGIC.len()] != MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid native cursor seed",
        ));
    }
    let expected = u32::from_le_bytes(contents[8..12].try_into().unwrap());
    let bytes = &contents[12..];
    if crc32fast::hash(bytes) != expected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "native cursor seed checksum mismatch",
        ));
    }
    let (saved_generation, cursors): (u64, Vec<NativeCursor>) = bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_limit(MAX_SEED_BYTES)
        .deserialize(bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if saved_generation != generation {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "native cursor seed generation mismatch",
        ));
    }
    validate_cursors(saved_generation, &cursors)?;
    Ok(cursors)
}

fn validate_cursors(generation: u64, cursors: &[NativeCursor]) -> io::Result<()> {
    if generation == 0
        || cursors
            .iter()
            .any(|cursor| cursor.mount.is_empty() || cursor.tree == 0 || cursor.since == 0)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid native cursor seed value",
        ));
    }
    for (index, cursor) in cursors.iter().enumerate() {
        if cursors[..index]
            .iter()
            .any(|previous| previous.mount == cursor.mount && previous.tree == cursor.tree)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "duplicate native cursor identity",
            ));
        }
    }
    Ok(())
}

pub fn seed_identity(cursors: &[NativeCursor]) -> io::Result<u32> {
    let mut ordered = cursors.iter().collect::<Vec<_>>();
    ordered.sort_by(|a, b| (&a.mount, a.tree).cmp(&(&b.mount, b.tree)));
    let bytes =
        bincode::serialize(&ordered).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(crc32fast::hash(&bytes))
}

/// Restore the cursor sidecar after a base was published but its seed was not.
pub fn publish_seed(staged: &Path, destination: &Path, generation: u64) -> io::Result<()> {
    match load_seed(staged, generation) {
        Ok(cursors) => save_seed(destination, generation, &cursors),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests;
