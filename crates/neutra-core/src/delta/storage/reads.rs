//! Point and bulk reads of the resolved overlay.

use super::*;

impl DeltaStore {
    pub(in crate::delta) fn get(&self, path: &str) -> io::Result<Option<FileRecord>> {
        let bytes: Option<Option<Vec<u8>>> = self
            .db()?
            .query_row("SELECT record FROM changes WHERE path=?1", [path], |row| {
                row.get(0)
            })
            .optional()
            .map_err(sql_error)?;
        bytes
            .flatten()
            .map(|bytes| bincode::deserialize(&bytes).map_err(sql_error))
            .transpose()
    }

    pub(in crate::delta) fn count(&self) -> io::Result<usize> {
        // The in-memory filter already knows the row count; a SQL count walks
        // the whole table on every search.
        if let Some(filter) = &self.shadow {
            return Ok(filter.len());
        }
        let count: i64 = self
            .db()?
            .query_row(
                "SELECT (SELECT count(*) FROM changes)+(SELECT count(*) FROM prefixes)",
                [],
                |r| r.get(0),
            )
            .map_err(sql_error)?;
        usize::try_from(count).map_err(|_| io::Error::other("delta change count overflow"))
    }

    pub(in crate::delta) fn each(
        &self,
        removed: bool,
        mut f: impl FnMut(&str, Option<FileRecord>) -> io::Result<()>,
    ) -> io::Result<()> {
        let mut statement = self
            .db()?
            .prepare(if removed {
                "SELECT path,NULL FROM changes WHERE removed=1 ORDER BY path"
            } else {
                "SELECT path,record FROM changes WHERE removed=0 ORDER BY path"
            })
            .map_err(sql_error)?;
        let mut rows = statement.query([]).map_err(sql_error)?;
        while let Some(row) = rows.next().map_err(sql_error)? {
            let path: String = row.get(0).map_err(sql_error)?;
            let bytes: Option<Vec<u8>> = row.get(1).map_err(sql_error)?;
            let record = bytes
                .map(|b| bincode::deserialize(&b).map_err(sql_error))
                .transpose()?;
            f(&path, record)?;
        }
        Ok(())
    }
}
