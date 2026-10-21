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
    let mut file = open_seed(&seed_path(base))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "native-cursor-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn roundtrip_requires_matching_generation_and_checksum() {
        let fixture = Fixture::new();
        let base = fixture.0.join("index.nsx");
        let cursors = vec![NativeCursor {
            mount: "/mnt/data".into(),
            tree: 257,
            since: 42,
        }];
        save_seed(&base, 91, &cursors).unwrap();
        assert_eq!(load_seed(&base, 91).unwrap(), cursors);
        assert_eq!(
            load_seed(&base, 92).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        let path = seed_path(&base);
        let mut bytes = fs::read(&path).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        fs::write(path, bytes).unwrap();
        assert_eq!(
            load_seed(&base, 91).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn rejects_zero_generation_and_duplicate_cursors() {
        let fixture = Fixture::new();
        let base = fixture.0.join("index.nsx");
        let cursor = NativeCursor {
            mount: "/mnt/data".into(),
            tree: 257,
            since: 42,
        };
        assert_eq!(
            save_seed(&base, 0, &[cursor.clone()]).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            save_seed(&base, 1, &[cursor.clone(), cursor])
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }
}

pub fn seed_identity(cursors: &[NativeCursor]) -> io::Result<u32> {
    let mut ordered = cursors.iter().collect::<Vec<_>>();
    ordered.sort_by(|a, b| (&a.mount, a.tree).cmp(&(&b.mount, b.tree)));
    let bytes =
        bincode::serialize(&ordered).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(crc32fast::hash(&bytes))
}
