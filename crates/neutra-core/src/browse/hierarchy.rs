use super::{sql_error, BrowserIndex};
use crate::FileRecord;
use rusqlite::{params, OptionalExtension};
use std::io;

impl BrowserIndex {
    /// Read direct children through path keys, jumping past each child's subtree.
    pub fn directory_children(&self, directory: &str) -> io::Result<Vec<FileRecord>> {
        // '/' is the catalog watcher root; trimming must preserve it as the
        // prefix while '0' (0x30) still sorts just after '/' (0x2f).
        let root = directory.trim_end_matches('/');
        let prefix = if root.is_empty() {
            "/".to_string()
        } else {
            format!("{root}/")
        };
        let end = if root.is_empty() {
            "0".to_string()
        } else {
            format!("{root}0")
        };
        let mut cursor = prefix.clone();
        let mut children = Vec::new();
        let mut decoded = None;
        let mut statement = self.db.prepare_cached(
            "SELECT path FROM entries INDEXED BY by_path WHERE path>=?1 AND path<?2 ORDER BY path LIMIT 1",
        ).map_err(sql_error)?;
        loop {
            let path: Option<String> = statement
                .query_row(params![cursor, end], |row| row.get(0))
                .optional()
                .map_err(sql_error)?;
            let Some(path) = path else { break };
            let relative = path.strip_prefix(&prefix).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "browse path escaped directory prefix",
                )
            })?;
            if let Some(slash) = relative.find('/') {
                // The subtree of this immediate child is contiguous in the B-tree.
                // The next sibling sorts immediately after the subtree because
                // '/' (0x2f) is advanced to '0' (0x30) in the key prefix.
                cursor = format!("{}{}0", prefix, &relative[..slash]);
            } else {
                if let Some(record) = self.record_by_path_cached(&path, &mut decoded)? {
                    children.push(record);
                }
                // Advance past exactly this key. NUL sorts below every valid
                // path byte, so this never skips a longer sibling ('ab' after
                // 'a') or a descendant, unlike a high-codepoint suffix.
                cursor = format!("{path}\0");
            }
        }
        Ok(children)
    }

    /// Read a bounded subtree; callers must reject overflow before committing moves.
    pub fn subtree_records(
        &self,
        directory: &str,
        limit: usize,
    ) -> io::Result<Option<Vec<FileRecord>>> {
        let root = directory.trim_end_matches('/');
        let prefix = if root.is_empty() {
            "/".to_string()
        } else {
            format!("{root}/")
        };
        let end = if root.is_empty() {
            "0".to_string()
        } else {
            format!("{root}0")
        };
        let count = limit
            .checked_add(1)
            .and_then(|n| i64::try_from(n).ok())
            .ok_or_else(|| io::Error::other("subtree limit overflow"))?;
        let mut statement = self.db.prepare_cached(
            "SELECT id,record FROM entries INDEXED BY by_path WHERE path>=?1 AND path<?2 ORDER BY path LIMIT ?3",
        ).map_err(sql_error)?;
        let mut rows = statement
            .query(params![prefix, end, count])
            .map_err(sql_error)?;
        let mut records = Vec::new();
        let mut block = None;
        let mut decoded = Vec::new();
        while let Some(row) = rows.next().map_err(sql_error)? {
            if records.len() == limit {
                return Ok(None);
            }
            let id: i64 = row.get(0).map_err(sql_error)?;
            let bytes: Option<Vec<u8>> = row.get(1).map_err(sql_error)?;
            let record = if let Some(bytes) = bytes {
                bincode::deserialize(&bytes).map_err(io::Error::other)?
            } else {
                if id <= 0 || id as u64 > self.base.len() {
                    return Err(io::Error::other("invalid browse record address"));
                }
                let address = (id - 1) as usize;
                let next_block = (address / crate::compact::BLOCK_RECORDS) as u32;
                if block != Some(next_block) {
                    decoded = self.base.read_block(next_block)?;
                    self.base.release_block(next_block);
                    block = Some(next_block);
                }
                decoded
                    .get(address % crate::compact::BLOCK_RECORDS)
                    .cloned()
                    .ok_or_else(|| io::Error::other("invalid browse record slot"))?
            };
            records.push(record);
        }
        Ok(Some(records))
    }

    #[cfg(test)]
    pub(super) fn hierarchy_path_order(&self) -> io::Result<Vec<String>> {
        let mut statement = self
            .db
            .prepare("SELECT path FROM entries INDEXED BY by_path ORDER BY path")
            .map_err(sql_error)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(sql_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(sql_error)
    }
}
