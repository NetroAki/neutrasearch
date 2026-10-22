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
        let mut cursor = Self {
            index,
            block: 0,
            records: Vec::new(),
            at: 0,
        };
        cursor.reseek(key)?;
        Ok(cursor)
    }

    /// Position on the first record at or after `key`.
    fn reseek(&mut self, key: &str) -> io::Result<()> {
        let (mut low, mut high) = (0usize, self.index.blocks.count);
        while low < high {
            let middle = low + (high - low) / 2;
            let records = self.index.read_block(middle as u32)?;
            if records
                .last()
                .is_some_and(|last| compare_index_paths(last.path.as_ref(), key) == Ordering::Less)
            {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        self.block = low;
        self.records = if low < self.index.blocks.count {
            self.index.read_block(low as u32)?
        } else {
            Vec::new()
        };
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
        if !cfg!(windows) && self.record_version < 7 {
            let mut out = Vec::new();
            for block_id in 0..self.blocks.count {
                for record in self.read_block(block_id as u32)? {
                    if starts_with_folded(record.path.as_ref(), &prefix)
                        && !record.path.as_ref()[prefix.len()..].contains('/')
                        && !record.path.as_ref()[prefix.len()..].contains('\\')
                    {
                        out.push(record);
                    }
                }
            }
            return Ok(out);
        }
        let mut cursor = Cursor::seek(self, &prefix)?;
        let mut out = Vec::new();
        while let Some(record) = cursor.peek() {
            let path = record.path.as_ref();
            if !starts_with_folded(path, &prefix) {
                break;
            }
            let rest = &path[prefix.len()..];
            let separator = if cfg!(windows) {
                rest.find(['/', '\\'])
            } else {
                rest.find('/')
            };
            match separator {
                None => {
                    out.push(record.clone());
                    cursor.advance()?;
                }
                Some(end) => {
                    // '0' sorts just after '/', so this key is past the whole subtree.
                    let past = format!("{prefix}{}0", &rest[..end]);
                    if compare_index_paths(&past, path) == Ordering::Greater {
                        cursor.reseek(&past)?;
                    } else {
                        cursor.advance()?;
                    }
                }
            }
        }
        Ok(out)
    }

    /// Direct children of `dir` as the base and delta show them together.
    pub fn directory_children(
        &self,
        dir: &str,
        delta: Option<&DeltaIndex>,
    ) -> io::Result<Vec<FileRecord>> {
        let prefix = dir_prefix(dir);
        let mut base = self.base_children(dir)?;
        if cfg!(windows) {
            base.retain(|record| {
                starts_with_folded(record.path.as_ref(), &prefix)
                    && !record.path.as_ref()[prefix.len()..].contains(['/', '\\'])
            });
        }
        let shadows = match delta {
            Some(delta) => {
                delta.shadows_batch(&base.iter().map(|r| r.path.as_ref()).collect::<Vec<_>>())?
            }
            None => vec![false; base.len()],
        };
        let mut out: Vec<FileRecord> = base
            .into_iter()
            .zip(shadows)
            .filter_map(|(r, shadowed)| (!shadowed).then_some(r))
            .collect();
        if let Some(delta) = delta {
            delta.for_each_upsert(|record| {
                let path = record.path.as_ref();
                if !starts_with_folded(path, &prefix) {
                    return Ok(());
                }
                let legacy = !cfg!(windows) && self.record_version < 7;
                let tail = &path[prefix.len()..];
                let slash_only = !cfg!(windows) && !legacy;
                let is_child = if slash_only {
                    !tail.contains('/')
                } else {
                    !tail.contains(['/', '\\'])
                };
                if is_child {
                    out.push(record);
                }
                Ok(())
            })?;
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
        self.require_current_order()?;
        let prefix = dir_prefix(dir);
        let mut cursor = Cursor::seek(self, &prefix)?;
        let mut out = Vec::new();
        while cursor
            .peek()
            .is_some_and(|r| starts_with_folded(r.path.as_ref(), &prefix))
        {
            let mut block = Vec::with_capacity(crate::compact::BLOCK_RECORDS);
            for _ in 0..crate::compact::BLOCK_RECORDS {
                let Some(record) = cursor.peek() else { break };
                if !starts_with_folded(record.path.as_ref(), &prefix) {
                    break;
                }
                block.push(record.clone());
                cursor.advance()?;
            }
            let shadows = match delta {
                Some(d) => {
                    d.shadows_batch(&block.iter().map(|r| r.path.as_ref()).collect::<Vec<_>>())?
                }
                None => vec![false; block.len()],
            };
            for (record, shadowed) in block.into_iter().zip(shadows) {
                if !shadowed {
                    if out.len() >= limit {
                        return Ok(None);
                    }
                    out.push(record);
                }
            }
        }
        let mut exceeded = false;
        if let Some(delta) = delta {
            delta.for_each_upsert(|record| {
                if starts_with_folded(record.path.as_ref(), &prefix) {
                    if out.len() >= limit {
                        exceeded = true;
                    } else {
                        out.push(record);
                    }
                }
                Ok(())
            })?;
        }
        if exceeded {
            return Ok(None);
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out.dedup_by(|a, b| a.path == b.path);
        Ok(Some(out))
    }
}

#[cfg(test)]
mod tests;
