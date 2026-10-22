use super::catalog::Tombstones;
use super::columns::BlockReader;
use super::{sql_error, BrowserIndex};
use crate::FileRecord;
use rusqlite::{params, OptionalExtension};
use std::{io, sync::Arc};

/// Ordered cursor over base rows (minus tombstones) and live rows.
struct Walk<'a> {
    browser: &'a BrowserIndex,
    reader: BlockReader<'a>,
    dead: Arc<Tombstones>,
}

impl<'a> Walk<'a> {
    fn new(browser: &'a BrowserIndex) -> io::Result<Self> {
        Ok(Self {
            browser,
            reader: BlockReader::with_cache(&browser.base, &browser.decoded_blocks),
            dead: browser.tombstones()?,
        })
    }

    fn base_at(&mut self, id: u32) -> io::Result<Option<FileRecord>> {
        let id = self.dead.next_live(id);
        if u64::from(id) >= self.browser.base.len() {
            return Ok(None);
        }
        Ok(Some(self.reader.record(id)?.clone()))
    }

    /// The first record whose path is at or after the cursor and before the end.
    fn first_from(&mut self, cursor: &str, end: &str) -> io::Result<Option<FileRecord>> {
        let id = self.reader.path_lower_bound(cursor)?;
        let base = self
            .base_at(id)?
            .filter(|record| record.path.as_ref() < end);
        let live: Option<Vec<u8>> = self
            .browser
            .db
            .prepare_cached(
                "SELECT record FROM entries INDEXED BY by_path WHERE path>=?1 AND path<?2 ORDER BY path LIMIT 1",
            )
            .map_err(sql_error)?
            .query_row(params![cursor, end], |row| row.get(0))
            .optional()
            .map_err(sql_error)?;
        let live = live
            .map(|bytes| bincode::deserialize::<FileRecord>(&bytes).map_err(io::Error::other))
            .transpose()?;
        Ok(match (base, live) {
            (Some(base), Some(live)) => Some(if live.path <= base.path { live } else { base }),
            (base, live) => base.or(live),
        })
    }
}

fn bounds(directory: &str) -> (String, String) {
    // '/' is the catalog watcher root; trimming must preserve it as the
    // prefix while '0' (0x30) still sorts just after '/' (0x2f).
    let root = directory.trim_end_matches('/');
    if root.is_empty() {
        ("/".to_string(), "0".to_string())
    } else {
        (format!("{root}/"), format!("{root}0"))
    }
}

impl BrowserIndex {
    /// Read direct children through path keys, jumping past each child's subtree.
    pub fn directory_children(&self, directory: &str) -> io::Result<Vec<FileRecord>> {
        let (prefix, end) = bounds(directory);
        let snapshot = self.read_snapshot()?;
        let mut walk = Walk::new(self)?;
        let mut cursor = prefix.clone();
        let mut children: Vec<FileRecord> = Vec::new();
        while let Some(record) = walk.first_from(&cursor, &end)? {
            let relative = record.path.strip_prefix(&prefix).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "browse path escaped directory prefix",
                )
            })?;
            if let Some(slash) = relative.find('/') {
                let child_path = format!("{}{}", prefix, &relative[..slash]);
                if children
                    .last()
                    .is_none_or(|child| child.path.as_ref() != child_path)
                {
                    let deeper = relative[slash + 1..].contains('/');
                    children.push(synthetic_child(record.clone(), child_path, deeper));
                }
                // The subtree of this immediate child is contiguous in path order.
                // The next sibling sorts immediately after the subtree because
                // '/' (0x2f) is advanced to '0' (0x30) in the key prefix.
                cursor = format!("{}{}0", prefix, &relative[..slash]);
            } else {
                // Advance past exactly this key. NUL sorts below every valid
                // path byte, so this never skips a longer sibling ('ab' after
                // 'a') or a descendant, unlike a high-codepoint suffix.
                cursor = format!("{}\0", record.path);
                children.push(record);
            }
        }
        drop(snapshot);
        Ok(children)
    }

    /// Read a bounded subtree; callers must reject overflow before committing moves.
    pub fn subtree_records(
        &self,
        directory: &str,
        limit: usize,
    ) -> io::Result<Option<Vec<FileRecord>>> {
        let (prefix, end) = bounds(directory);
        self.range_records(&prefix, Some(&end), limit)
    }

    /// Records with start <= path < end in path order, or None past the limit.
    fn range_records(
        &self,
        start: &str,
        end: Option<&str>,
        limit: usize,
    ) -> io::Result<Option<Vec<FileRecord>>> {
        let count = limit
            .checked_add(1)
            .and_then(|n| i64::try_from(n).ok())
            .ok_or_else(|| io::Error::other("subtree limit overflow"))?;
        let snapshot = self.read_snapshot()?;
        let mut walk = Walk::new(self)?;
        let mut id = walk.reader.path_lower_bound(start)?;
        let high = match end {
            Some(end) => walk.reader.path_lower_bound(end)?,
            None => self.base.len() as u32,
        };
        let mut live = Vec::new();
        {
            let mut statement = self
                .db
                .prepare_cached(
                    "SELECT record FROM entries INDEXED BY by_path WHERE path>=?1 AND (?2 IS NULL OR path<?2) ORDER BY path LIMIT ?3",
                )
                .map_err(sql_error)?;
            let mut rows = statement
                .query(params![start, end, count])
                .map_err(sql_error)?;
            while let Some(row) = rows.next().map_err(sql_error)? {
                let bytes: Vec<u8> = row.get(0).map_err(sql_error)?;
                live.push(bincode::deserialize::<FileRecord>(&bytes).map_err(io::Error::other)?);
            }
        }
        let mut live = live.into_iter().peekable();
        let mut records = Vec::new();
        loop {
            id = walk.dead.next_live(id);
            let base = if id < high {
                Some(walk.reader.record(id)?.clone())
            } else {
                None
            };
            let take_live = match (&base, live.peek()) {
                (None, None) => break,
                (Some(base), Some(next)) => next.path <= base.path,
                (None, Some(_)) => true,
                (Some(_), None) => false,
            };
            if records.len() == limit {
                return Ok(None);
            }
            if take_live {
                records.extend(live.next());
            } else {
                records.extend(base);
                id += 1;
            }
        }
        drop(snapshot);
        Ok(Some(records))
    }

    #[cfg(test)]
    pub(super) fn hierarchy_path_order(&self) -> io::Result<Vec<String>> {
        let all = self.range_records("", None, 1 << 40)?.unwrap_or_default();
        Ok(all.into_iter().map(|record| record.path.into()).collect())
    }
}

fn synthetic_child(
    mut child: FileRecord,
    child_path: String,
    has_deeper_parent: bool,
) -> FileRecord {
    // A live file can precede its folder's first metadata event.
    child.native_id = if has_deeper_parent {
        0
    } else {
        child.native_parent
    };
    child.native_parent = 0;
    child.path = child_path.into_boxed_str();
    child.kind = crate::FileKind::Dir;
    child.size = 0;
    child.disk = FileRecord::allocated_bytes(0);
    child.mtime = 0;
    child.mode = 0;

    child
}
