use super::*;
use std::collections::HashSet;

pub(super) fn ancestors(path: &str) -> impl Iterator<Item = &str> {
    std::iter::once(path).chain(path.match_indices('/').map(move |(position, _)| {
        if position == 0 {
            &path[..1]
        } else if position == 2 && path.as_bytes()[1] == b':' {
            &path[..3]
        } else {
            &path[..position]
        }
    }))
}

pub(super) fn apply(tx: &rusqlite::Transaction<'_>, path: &str) -> io::Result<()> {
    let prefix = format!("{}/", path.trim_end_matches('/'));
    let mut end = prefix.clone();
    end.pop();
    end.push('0');
    tx.execute(
        "DELETE FROM changes WHERE path=?1 OR (path>=?2 AND path<?3)",
        params![path, prefix, end],
    )
    .map_err(sql_error)?;
    tx.execute(
        "DELETE FROM prefixes WHERE path=?1 OR (path>=?2 AND path<?3)",
        params![path, prefix, end],
    )
    .map_err(sql_error)?;
    let mut statement = tx
        .prepare_cached("SELECT EXISTS(SELECT 1 FROM prefixes WHERE path=?1)")
        .map_err(sql_error)?;
    for ancestor in ancestors(path) {
        if statement
            .query_row([ancestor], |row| row.get::<_, bool>(0))
            .map_err(sql_error)?
        {
            return Ok(());
        }
    }
    tx.execute("INSERT INTO prefixes VALUES(?1)", [path])
        .map_err(sql_error)?;
    Ok(())
}

impl DeltaStore {
    pub(in crate::delta) fn is_removed(&self, path: &str) -> io::Result<bool> {
        let exact: Option<bool> = self
            .db()?
            .query_row("SELECT removed FROM changes WHERE path=?1", [path], |row| {
                row.get(0)
            })
            .optional()
            .map_err(sql_error)?;
        if let Some(removed) = exact {
            return Ok(removed);
        }
        self.covered(path)
    }

    fn covered(&self, path: &str) -> io::Result<bool> {
        let mut statement = self
            .db()?
            .prepare_cached("SELECT EXISTS(SELECT 1 FROM prefixes WHERE path=?1)")
            .map_err(sql_error)?;
        for ancestor in ancestors(path) {
            if statement
                .query_row([ancestor], |row| row.get::<_, bool>(0))
                .map_err(sql_error)?
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(in crate::delta) fn shadows(&self, path: &str) -> io::Result<bool> {
        let exact = self
            .db()?
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM changes WHERE path=?1)",
                [path],
                |row| row.get::<_, bool>(0),
            )
            .map_err(sql_error)?;
        if exact {
            return Ok(true);
        }
        self.covered(path)
    }

    pub(in crate::delta) fn each_prefix(
        &self,
        visit: &mut impl FnMut(Box<str>) -> io::Result<()>,
    ) -> io::Result<()> {
        let mut statement = self
            .db()?
            .prepare("SELECT path FROM prefixes ORDER BY path")
            .map_err(sql_error)?;
        let mut rows = statement.query([]).map_err(sql_error)?;
        while let Some(row) = rows.next().map_err(sql_error)? {
            visit(row.get::<_, String>(0).map_err(sql_error)?.into_boxed_str())?;
        }
        Ok(())
    }

    fn prefix_matches(&self, paths: &[&str]) -> io::Result<HashSet<String>> {
        let mut matches = HashSet::new();
        let mut ancestors = paths.iter().flat_map(|path| ancestors(path));
        loop {
            let batch: Vec<_> = ancestors.by_ref().take(384).collect();
            if batch.is_empty() {
                return Ok(matches);
            }
            lookup(self.db()?, "prefixes", &batch, &mut matches)?;
        }
    }

    /// The in-memory filter, loaded on first use. Callers query it after
    /// releasing the overlay lock so parallel scans do not serialize.
    pub(in crate::delta) fn shadow_filter(&mut self) -> io::Result<std::sync::Arc<ShadowFilter>> {
        if self.shadow.is_none() {
            self.shadow = Some(std::sync::Arc::new(ShadowFilter::load(self.db()?)?));
        }
        Ok(self.shadow.clone().expect("filter loaded"))
    }

    pub(in crate::delta) fn shadows_few(&self, paths: &[&str]) -> io::Result<Vec<bool>> {
        self.shadows_exact(paths)
    }

    fn shadows_exact(&self, paths: &[&str]) -> io::Result<Vec<bool>> {
        let mut out = Vec::with_capacity(paths.len());
        let has_prefixes = self
            .db()?
            .query_row("SELECT EXISTS(SELECT 1 FROM prefixes)", [], |row| {
                row.get::<_, bool>(0)
            })
            .map_err(sql_error)?;
        for chunk in paths.chunks(384) {
            let mut exact = HashSet::new();
            lookup(self.db()?, "changes", chunk, &mut exact)?;
            let removed = if has_prefixes {
                self.prefix_matches(chunk)?
            } else {
                HashSet::new()
            };
            out.extend(chunk.iter().map(|path| {
                exact.contains(*path) || ancestors(path).any(|ancestor| removed.contains(ancestor))
            }));
        }
        Ok(out)
    }
}

fn lookup(
    db: &Connection,
    table: &str,
    paths: &[&str],
    out: &mut HashSet<String>,
) -> io::Result<()> {
    let marks = std::iter::repeat_n("?", paths.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!("SELECT path FROM {table} WHERE path IN ({marks})");
    let mut statement = db.prepare_cached(&sql).map_err(sql_error)?;
    let rows = statement
        .query_map(rusqlite::params_from_iter(paths.iter()), |row| {
            row.get::<_, String>(0)
        })
        .map_err(sql_error)?;
    for row in rows {
        out.insert(row.map_err(sql_error)?);
    }
    Ok(())
}
