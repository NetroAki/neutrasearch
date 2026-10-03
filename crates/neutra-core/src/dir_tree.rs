//! Shallow folder summary for the tree view: totals and children for the top
//! few levels of every source, readable in milliseconds. It is derived in one
//! streaming pass over the directory-summary sidecar and is bound to the same
//! compact-index generation, so a stale file is never served.

use crate::dir_summary::{directory_summary_path, open_private_file, walk_frames};
use crate::{DirFile, DirListing, DirSubdir, FileKind};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{self, Write};
use std::io::Read;
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 8] = b"NEUTREE1";
const PREFIX: usize = 16;
/// Folders up to this depth ("/" is 0) keep their children listed.
const MAX_DEPTH: usize = 3;
const MAX_FILE_ROWS: usize = 2000;

#[derive(Serialize, Deserialize)]
struct Folder {
    path: String,
    size: u64,
    logical: u64,
    files: u64,
    subdirs: Vec<Sub>,
    rows: Vec<Row>,
    truncated: bool,
}

#[derive(Serialize, Deserialize)]
struct Sub {
    name: String,
    size: u64,
    logical: u64,
    files: u64,
}

#[derive(Serialize, Deserialize)]
struct Row {
    name: String,
    size: u64,
    logical: u64,
    kind: FileKind,
}

#[derive(Default)]
struct Acc {
    size: u64,
    logical: u64,
    files: u64,
    subs: HashMap<String, (u64, u64, u64)>,
    rows: Vec<Row>,
    truncated: bool,
}

pub struct TreeSummary {
    folders: HashMap<String, Folder>,
}

impl TreeSummary {
    pub fn path_for(index_path: &Path) -> PathBuf {
        let mut value = index_path.as_os_str().to_os_string();
        value.push(".tree");
        value.into()
    }

    fn header_generation(bytes: &[u8]) -> io::Result<u64> {
        if bytes.len() < PREFIX || &bytes[..8] != MAGIC {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "not a tree summary"));
        }
        Ok(u64::from_le_bytes(bytes[8..16].try_into().unwrap()))
    }

    /// Build the summary for `generation` unless a current one is on disk.
    pub fn ensure(index_path: &Path, generation: u64) -> io::Result<()> {
        let current = std::fs::File::open(Self::path_for(index_path)).and_then(|mut file| {
            let mut header = [0u8; PREFIX];
            file.read_exact(&mut header)?;
            Self::header_generation(&header)
        });
        if current.is_ok_and(|found| found == generation) {
            return Ok(());
        }
        Self::build(index_path, generation)
    }

    pub fn open_for_compact(index_path: &Path, generation: u64) -> io::Result<Self> {
        let bytes = std::fs::read(Self::path_for(index_path))?;
        if Self::header_generation(&bytes)? != generation {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "tree summary is stale"));
        }
        let raw = zstd::stream::decode_all(&bytes[PREFIX..])?;
        let folders: Vec<Folder> = bincode::deserialize(&raw)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        Ok(Self {
            folders: folders.into_iter().map(|f| (f.path.clone(), f)).collect(),
        })
    }

    /// The folder as a `DirListing`, or `None` when it is deeper than the
    /// summary covers.
    pub fn listing(&self, dir: &str) -> Option<DirListing> {
        let key = if dir == "/" { dir } else { dir.trim_end_matches('/') };
        let folder = self.folders.get(key)?;
        Some(DirListing {
            files: folder
                .rows
                .iter()
                .map(|row| DirFile {
                    name: row.name.as_str().into(),
                    size: row.size,
                    logical: row.logical,
                    kind: row.kind,
                })
                .collect(),
            subdirs: folder
                .subdirs
                .iter()
                .map(|sub| DirSubdir {
                    name: sub.name.as_str().into(),
                    size: sub.size,
                    logical: sub.logical,
                    files: sub.files,
                })
                .collect(),
            total_size: folder.size,
            total_logical: folder.logical,
            total_count: folder.files,
            files_truncated: folder.truncated,
        })
    }

    fn build(index_path: &Path, generation: u64) -> io::Result<()> {
        let file = std::fs::File::open(directory_summary_path(index_path))?;
        // SAFETY: read-only mapping of a file that is only ever replaced
        // atomically by rename.
        let map = unsafe { memmap2::Mmap::map(&file)? };
        let legacy = map.get(8..12).is_some_and(|v| u32::from_le_bytes(v.try_into().unwrap()) == 1);
        // Version 1 sidecars carry no on-disk totals; fall back to apparent size.
        let size_of = |physical: u64, logical: u64| if legacy { logical } else { physical };
        let mut accs: HashMap<String, Acc> = HashMap::new();
        walk_frames(&map, &mut |entry| {
            if depth(&entry.path) > MAX_DEPTH {
                return Ok(());
            }
            let acc = accs.entry(entry.path.to_string()).or_default();
            acc.size += size_of(entry.physical_bytes, entry.logical_bytes);
            acc.logical += entry.logical_bytes;
            acc.files += entry.file_count;
            for child in &entry.children {
                let name = child.path.rsplit('/').next().unwrap_or_default().to_owned();
                let size = size_of(child.physical_bytes, child.logical_bytes);
                if child.kind == FileKind::Dir {
                    let sub = acc.subs.entry(name).or_default();
                    sub.0 += size;
                    sub.1 += child.logical_bytes;
                    sub.2 += child.file_count;
                } else {
                    acc.rows.push(Row { name, size, logical: child.logical_bytes, kind: child.kind });
                    if acc.rows.len() >= MAX_FILE_ROWS * 2 {
                        trim_rows(acc);
                    }
                }
            }
            Ok(())
        })?;
        let mut folders: Vec<Folder> = accs
            .into_iter()
            .map(|(path, mut acc)| {
                trim_rows(&mut acc);
                let mut subdirs: Vec<Sub> = acc
                    .subs
                    .into_iter()
                    .map(|(name, (size, logical, files))| Sub { name, size, logical, files })
                    .collect();
                subdirs.sort_unstable_by_key(|sub| std::cmp::Reverse(sub.size));
                Folder {
                    path,
                    size: acc.size,
                    logical: acc.logical,
                    files: acc.files,
                    subdirs,
                    rows: acc.rows,
                    truncated: acc.truncated,
                }
            })
            .collect();
        folders.sort_unstable_by(|a, b| a.path.cmp(&b.path));
        let payload = bincode::serialize(&folders)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let destination = Self::path_for(index_path);
        let mut staged = destination.as_os_str().to_os_string();
        staged.push(format!(".tmp-{}", std::process::id()));
        let staged = PathBuf::from(staged);
        let mut out = open_private_file(&staged)?;
        out.write_all(MAGIC)?;
        out.write_all(&generation.to_le_bytes())?;
        out.write_all(&zstd::stream::encode_all(&payload[..], 3)?)?;
        out.sync_all()?;
        drop(out);
        std::fs::rename(staged, destination)
    }
}

fn depth(path: &str) -> usize {
    path.split('/').filter(|part| !part.is_empty()).count()
}

/// Keep the largest direct files; the rest are only counted in the totals.
fn trim_rows(acc: &mut Acc) {
    if acc.rows.len() > MAX_FILE_ROWS {
        acc.rows.sort_unstable_by_key(|row| std::cmp::Reverse(row.size));
        acc.rows.truncate(MAX_FILE_ROWS);
        acc.truncated = true;
    } else {
        acc.rows.sort_unstable_by_key(|row| std::cmp::Reverse(row.size));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dir_summary::DirectorySummary;
    use crate::{CompactIndex, FileRecord, FsKind};

    fn rec(path: &str, size: u64, disk: u64, kind: FileKind, source: u32) -> FileRecord {
        FileRecord {
            path: path.into(),
            size,
            mtime: 0,
            mode: 0,
            kind,
            fs: FsKind::Btrfs,
            native_id: size + 100,
            native_parent: 0,
            source,
            disk,
        }
    }

    #[test]
    fn tree_listing_matches_the_streamed_listing_across_sources() {
        let records = vec![
            rec("/etc/a.conf", 10, 0, FileKind::File, 0),
            rec("/mnt/own.txt", 7, 0, FileKind::File, 0),
            rec("/home/u/doc/a.md", 3, 0, FileKind::File, 0),
            rec("/home/u/empty", 0, 0, FileKind::Dir, 0),
            rec("/mnt/keep/deep/song.wav", 1000, 600, FileKind::File, 1),
            rec("/mnt/keep/x.txt", 5, 0, FileKind::File, 1),
        ];
        let path = std::env::temp_dir().join(format!("neutra-tree-{}.idx", std::process::id()));
        CompactIndex::build(&records, &path).unwrap();
        let index = CompactIndex::open_fast(&path).unwrap();
        let generation = index.generation();
        DirectorySummary::build(&records, &path, generation).unwrap();
        TreeSummary::ensure(&path, generation).unwrap();
        let tree = TreeSummary::open_for_compact(&path, generation).unwrap();
        let shape = |listing: &DirListing| {
            let mut subs: Vec<_> = listing
                .subdirs
                .iter()
                .map(|s| (s.name.to_string(), s.size, s.logical, s.files))
                .collect();
            subs.sort();
            let mut files: Vec<_> = listing
                .files
                .iter()
                .map(|f| (f.name.to_string(), f.size, f.logical))
                .collect();
            files.sort();
            (listing.total_size, listing.total_logical, listing.total_count, subs, files)
        };
        for dir in ["/", "/mnt", "/home", "/home/u", "/mnt/keep"] {
            let want = index.list_directory(dir, None, None).unwrap();
            let got = tree.listing(dir).unwrap();
            assert_eq!(shape(&got), shape(&want), "listing for {dir}");
        }
        assert!(tree.listing("/mnt/keep/deep/inner/x").is_none());
        drop(index);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(directory_summary_path(&path));
        let _ = std::fs::remove_file(TreeSummary::path_for(&path));
    }
}
