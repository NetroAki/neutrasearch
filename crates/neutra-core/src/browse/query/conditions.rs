use super::*;

pub(in crate::browse) fn filters(query: &Query) -> (String, Vec<Value>) {
    let mut clauses = Vec::new();
    let mut values = Vec::new();
    if !query.kinds.is_empty() {
        clauses.push(format!("kind IN ({})", placeholders(query.kinds.len())));
        values.extend(query.kinds.iter().map(|kind| Value::Integer(*kind as i64)));
    }
    if !query.exts.is_empty() {
        clauses.push(format!("ext IN ({})", placeholders(query.exts.len())));
        values.extend(
            query
                .exts
                .iter()
                .map(|ext| Value::Text(ext.to_ascii_lowercase())),
        );
    }
    if !query.fss.is_empty() {
        clauses.push(format!("fs IN ({})", placeholders(query.fss.len())));
        values.extend(query.fss.iter().map(|fs| Value::Text(fs.label())));
    }
    if query.executable_only {
        clauses.push("executable=1".into());
    }
    for (bound, operator) in [(query.min_size, ">="), (query.max_size, "<=")] {
        if let Some(bound) = bound {
            clauses.push(format!("bytes {operator} ?"));
            values.push(Value::Blob(bound.to_be_bytes().to_vec()));
        }
    }
    if !query.scope_roots.is_empty() && query.scope_roots != ["/"] {
        let scopes: Vec<_> = query
            .scope_roots
            .iter()
            .map(|root| scope_clause(root, query.scope_case_sensitive, &mut values))
            .collect();
        clauses.push(format!("({})", scopes.join(" OR ")));
    }
    if let Some(root) = &query.under {
        clauses.push(scope_clause(root, false, &mut values));
    }
    for root in &query.exclude_roots {
        clauses.push(format!(
            "NOT {}",
            scope_clause(root, query.scope_case_sensitive, &mut values)
        ));
    }
    (
        if clauses.is_empty() {
            "1".into()
        } else {
            clauses.join(" AND ")
        },
        values,
    )
}

fn placeholders(count: usize) -> String {
    vec!["?"; count].join(",")
}

pub(super) fn scope_clause(root: &str, sensitive: bool, values: &mut Vec<Value>) -> String {
    let path = if sensitive && root.starts_with('/') {
        "path"
    } else if sensitive {
        "replace(path,char(92),'/')"
    } else if root.starts_with('/') && !cfg!(windows) {
        "lower(path)"
    } else {
        "lower(replace(path,char(92),'/'))"
    };
    let normalized = if root.starts_with('/') && !cfg!(windows) {
        root.to_owned()
    } else {
        root.replace('\\', "/")
    };
    let root = if sensitive {
        normalized
    } else {
        normalized.to_ascii_lowercase()
    };
    let prefix = format!("{}/", root.trim_end_matches('/'));
    let mut past = prefix.clone();
    past.pop();
    past.push('0');
    values.extend([Value::Text(root), Value::Text(prefix), Value::Text(past)]);
    format!("({path}=? OR ({path}>=? AND {path}<?))")
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;
    #[test]
    fn unix_literal_backslash_scopes_keep_their_spelling() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE entries(path TEXT); INSERT INTO entries VALUES('/allowed/name\\/keep.txt'),('/allowed/name/other.txt');").unwrap();
        let mut values = Vec::new();
        let clause = scope_clause("/allowed/name\\", true, &mut values);
        let paths: Vec<String> = db
            .prepare(&format!("SELECT path FROM entries WHERE {clause}"))
            .unwrap()
            .query_map(rusqlite::params_from_iter(values.iter()), |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(paths, ["/allowed/name\\/keep.txt"]);
    }
}
