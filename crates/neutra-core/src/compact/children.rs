//! Directory-shaped reads on the path-ordered base: direct children without
//! visiting their subtrees, and whole subtrees, both merged with the delta.
//! Blocks are sorted by folded path, so everything under `D/x/` lies in one
//! run; a child's subtree is skipped with one binary search.

use super::{compare_index_paths, dir_prefix, starts_with_folded, CompactIndex};
use crate::{DeltaIndex, FileRecord};
use std::cmp::Ordering;
use std::io;

/// A forward cursor over records in path order.
struct Cursor<'a> {
    index: &'a CompactIndex,
    block: usize,
    records: Vec<FileRecord>,
    at: usize,
}

impl<'a> Cursor<'a> {
    fn seek(index: &'a CompactIndex, key: &str) -> io::Result<Self> {
        let mut cursor = Self { index, block: 0, records: Vec::new(), at: 0 };
        cursor.reseek(key)?;
        Ok(cursor)
    }

    /// Position on the first record at or after `key`.
    fn reseek(&mut self, key: &str) -> io::Result<()> {
        let (mut low, mut high) = (0usize, self.index.blocks.count);
        while low < high {
            let middle = low + (high - low) / 2;
            let records = self.index.read_block(middle as u32)?;
            self.index.release_block(middle as u32);
            if records.last().is_some_and(|last| compare_index_paths(last.path.as_ref(), key) == Ordering::Less) {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        self.block = low;
        self.records = if low < self.index.blocks.count { self.index.read_block(low as u32)? } else { Vec::new() };
        self.at = self
            .records
            .iter()
            .position(|record| compare_index_paths(record.path.as_ref(), key) != Ordering::Less)
            .unwrap_or(self.records.len());
        self.settle()
    }

    /// Move to the next block when the current one is used up.
    fn settle(&mut self) -> io::Result<()> {
        while self.at >= self.records.len() && self.block + 1 < self.index.blocks.count {
            self.index.release_block(self.block as u32);
            self.block += 1;
            self.records = self.index.read_block(self.block as u32)?;
            self.at = 0;
        }
        Ok(())
    }

    fn peek(&self) -> Option<&FileRecord> {
        self.records.get(self.at)
    }

    fn advance(&mut self) -> io::Result<()> {
        self.at += 1;
        self.settle()
    }
}

impl CompactIndex {
    /// Direct children of `dir` in the base, with no delta applied.
    fn base_children(&self, dir: &str) -> io::Result<Vec<FileRecord>> {
        let prefix = dir_prefix(dir);
        let mut cursor = Cursor::seek(self, &prefix)?;
        let mut out = Vec::new();
        while let Some(record) = cursor.peek() {
            let path = record.path.as_ref();
            if !starts_with_folded(path, &prefix) {
                break;
            }
            let rest = &path[prefix.len()..];
            match rest.find(['/', '\\']) {
                None => {
                    out.push(record.clone());
                    cursor.advance()?;
                }
                Some(end) => {
                    // '0' sorts just after '/', so this key is past the whole subtree.
                    let past = format!("{prefix}{}0", &rest[..end]);
                    cursor.reseek(&past)?;
                }
            }
        }
        Ok(out)
    }

    /// Direct children of `dir` as the base and delta show them together.
    pub fn directory_children(&self, dir: &str, delta: Option<&DeltaIndex>) -> io::Result<Vec<FileRecord>> {
        let prefix = dir_prefix(dir);
        let mut out: Vec<FileRecord> = self
            .base_children(dir)?
            .into_iter()
            .filter(|record| !delta.is_some_and(|d| d.shadows(record.path.as_ref())))
            .collect();
        for record in delta.into_iter().flat_map(DeltaIndex::upserts) {
            let path = record.path.as_ref();
            if starts_with_folded(path, &prefix) && !path[prefix.len()..].contains(['/', '\\']) {
                out.push(record.clone());
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out.dedup_by(|a, b| a.path == b.path);
        Ok(out)
    }

    /// Every record strictly under `dir`, base and delta together, or
    /// `None` when more than `limit` exist.
    pub fn subtree_records(
        &self,
        dir: &str,
        delta: Option<&DeltaIndex>,
        limit: usize,
    ) -> io::Result<Option<Vec<FileRecord>>> {
        let prefix = dir_prefix(dir);
        let mut cursor = Cursor::seek(self, &prefix)?;
        let mut out = Vec::new();
        while let Some(record) = cursor.peek() {
            if !starts_with_folded(record.path.as_ref(), &prefix) {
                break;
            }
            if !delta.is_some_and(|d| d.shadows(record.path.as_ref())) {
                if out.len() >= limit {
                    return Ok(None);
                }
                out.push(record.clone());
            }
            cursor.advance()?;
        }
        for record in delta.into_iter().flat_map(DeltaIndex::upserts) {
            if starts_with_folded(record.path.as_ref(), &prefix) {
                if out.len() >= limit {
                    return Ok(None);
                }
                out.push(record.clone());
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out.dedup_by(|a, b| a.path == b.path);
        Ok(Some(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DeltaChange, FileKind, FsKind};

    fn rec(path: &str, kind: FileKind, id: u64) -> FileRecord {
        FileRecord {
            path: path.into(),
            size: 1,
            disk: 1,
            mtime: 5,
            mode: 0,
            kind,
            fs: FsKind::Btrfs,
            native_id: id,
            native_parent: 0,
            source: 0,
        }
    }

    fn fixture(name: &str) -> (std::path::PathBuf, CompactIndex) {
        let records = vec![
            rec("/d", FileKind::Dir, 1),
            rec("/d/a", FileKind::Dir, 2),
            rec("/d/a-b.txt", FileKind::File, 3),
            rec("/d/a/deep", FileKind::Dir, 4),
            rec("/d/a/deep/x.txt", FileKind::File, 5),
            rec("/d/a.txt", FileKind::File, 6),
            rec("/d/a0", FileKind::File, 7),
            rec("/d/b", FileKind::Dir, 8),
            rec("/d/b/y.txt", FileKind::File, 9),
            rec("/e/z.txt", FileKind::File, 10),
        ];
        let path = std::env::temp_dir().join(format!("neutra-children-{name}-{}.idx", std::process::id()));
        CompactIndex::build(&records, &path).unwrap();
        let index = CompactIndex::open_fast(&path).unwrap();
        (path, index)
    }

    fn names(records: &[FileRecord]) -> Vec<String> {
        records.iter().map(|r| r.path.to_string()).collect()
    }

    #[test]
    fn children_skip_subtrees_and_keep_odd_neighbours() {
        let (path, index) = fixture("kids");
        let kids = index.directory_children("/d", None).unwrap();
        assert_eq!(names(&kids), ["/d/a", "/d/a-b.txt", "/d/a.txt", "/d/a0", "/d/b"]);
        assert!(index.directory_children("/nothing", None).unwrap().is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn subtree_lists_everything_below_and_honours_the_limit() {
        let (path, index) = fixture("tree");
        let all = index.subtree_records("/d/a", None, 100).unwrap().unwrap();
        assert_eq!(names(&all), ["/d/a/deep", "/d/a/deep/x.txt"]);
        assert!(index.subtree_records("/d", None, 3).unwrap().is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_delta_adds_and_hides_children() {
        let (path, index) = fixture("delta");
        let delta_path = path.with_extension("delta");
        let mut delta = DeltaIndex::open(&delta_path, index.generation()).unwrap();
        delta.apply(DeltaChange::Remove("/d/b".into())).unwrap();
        delta.apply(DeltaChange::Upsert(rec("/d/new.txt", FileKind::File, 11))).unwrap();
        delta.apply(DeltaChange::Upsert(rec("/d/b/y2.txt", FileKind::File, 12))).unwrap();
        let kids = index.directory_children("/d", Some(&delta)).unwrap();
        assert_eq!(names(&kids), ["/d/a", "/d/a-b.txt", "/d/a.txt", "/d/a0", "/d/new.txt"]);
        let below = index.subtree_records("/d/b", Some(&delta), 10).unwrap().unwrap();
        // A tombstone hides only its own path; callers expand a removed
        // directory into one tombstone per descendant.
        assert_eq!(names(&below), ["/d/b/y.txt", "/d/b/y2.txt"]);
        drop(delta);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&delta_path);
    }
}
