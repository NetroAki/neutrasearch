//! Parallel scan of the overlay's live upserts. One reader connection per key
//! range keeps broad searches from walking millions of rows on one thread.

use super::*;
use crate::record_ref::RecordRef;

pub(in crate::delta) type KeyRange = (Option<String>, Option<String>);

impl DeltaStore {
    /// The database file and about `parts` key ranges covering every path.
    pub(in crate::delta) fn upsert_ranges(
        &mut self,
        parts: usize,
    ) -> io::Result<(PathBuf, Vec<KeyRange>)> {
        if self.shadow.is_none() {
            self.shadow = Some(std::sync::Arc::new(ShadowFilter::load(self.db()?)?));
        }
        let points = self
            .shadow
            .as_ref()
            .map(|filter| filter.split_points(parts))
            .unwrap_or_default();
        let mut ranges = Vec::with_capacity(points.len() + 1);
        let mut low: Option<String> = None;
        for point in points {
            ranges.push((low.take(), Some(point.clone())));
            low = Some(point);
        }
        ranges.push((low, None));
        Ok((self.path.clone(), ranges))
    }
}

/// Visit the live upserts inside one key range on a private connection.
pub(in crate::delta) fn scan_range(
    db_path: &Path,
    range: &KeyRange,
    mut visit: impl FnMut(&RecordRef<'_>) -> io::Result<()>,
) -> io::Result<()> {
    let db = crate::sqlite::open(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(sql_error)?;
    db.busy_timeout(std::time::Duration::from_secs(60))
        .map_err(sql_error)?;
    // Map the file for the length of the scan: every worker then reads pages
    // straight from the kernel cache instead of contending on SQLite's
    // shared allocator. The mapping ends when the connection drops.
    db.execute_batch("PRAGMA mmap_size=4294967296; PRAGMA cache_size=-1024;")
        .map_err(sql_error)?;
    let (sql, bounds): (&str, Vec<&str>) = match (&range.0, &range.1) {
        (None, None) => ("SELECT record FROM changes WHERE removed=0", vec![]),
        (Some(low), None) => (
            "SELECT record FROM changes WHERE removed=0 AND path>=?1",
            vec![low],
        ),
        (None, Some(high)) => (
            "SELECT record FROM changes WHERE removed=0 AND path<?1",
            vec![high],
        ),
        (Some(low), Some(high)) => (
            "SELECT record FROM changes WHERE removed=0 AND path>=?1 AND path<?2",
            vec![low, high],
        ),
    };
    let mut statement = db.prepare(sql).map_err(sql_error)?;
    let mut rows = statement
        .query(rusqlite::params_from_iter(bounds))
        .map_err(sql_error)?;
    while let Some(row) = rows.next().map_err(sql_error)? {
        let blob = row
            .get_ref(0)
            .map_err(sql_error)?
            .as_blob()
            .map_err(sql_error)?;
        let view: RecordRef<'_> = bincode::deserialize(blob).map_err(sql_error)?;
        visit(&view)?;
    }
    Ok(())
}
