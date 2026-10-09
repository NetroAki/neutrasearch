use super::{sql_error, BrowserIndex};
use crate::matcher::compare_records;
use crate::{CompactIndex, DeltaIndex, FileRecord, Query, SearchHit, SearchStats, SortKey};
use rusqlite::types::Value;
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::io;
use std::time::Instant;

pub(super) fn search(
    browser: &BrowserIndex,
    base: &CompactIndex,
    query: &Query,
    delta: Option<&DeltaIndex>,
    offset: usize,
    after: Option<&FileRecord>,
) -> io::Result<(Vec<SearchHit>, SearchStats)> {
    if !query.terms.is_empty() || query.regex.is_some() || query.limit == 0 {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "browse orders require a bounded term-free query",
        ));
    }
    let started = Instant::now();
    let (primary_order, index) = ordering(query.sort);
    let order = if matches!(
        query.sort,
        SortKey::NameAsc | SortKey::NameDesc | SortKey::PathAsc | SortKey::PathDesc
    ) {
        primary_order.to_owned()
    } else {
        format!("{primary_order},path {}", tie_path_direction(query.sort))
    };
    let index = if small_scope(browser, query)? {
        "by_path"
    } else {
        index
    };
    let (mut conditions, mut values) = filters(query);
    if let Some(record) = after {
        conditions.push_str(&format!(
            " AND {}",
            cursor_clause(query.sort, record, &mut values)
        ));
    }
    let take = query
        .limit
        .checked_add(offset)
        .ok_or_else(|| io::Error::other("page offset overflow"))?;
    let candidates = take
        .checked_add(1)
        .and_then(|count| i64::try_from(count).ok())
        .ok_or_else(|| io::Error::other("page limit overflow"))?;
    values.push(Value::Integer(candidates));
    let sql = format!("SELECT id,record FROM entries INDEXED BY {index} WHERE {conditions} ORDER BY {order} LIMIT ?");
    let mut statement = browser.db.prepare(&sql).map_err(sql_error)?;
    let mut rows = statement
        .query(rusqlite::params_from_iter(values))
        .map_err(sql_error)?;
    let matcher = query.matcher()?;
    let mut decoded: std::collections::HashMap<u32, Vec<FileRecord>> =
        std::collections::HashMap::new();
    let mut shadows: std::collections::HashMap<u32, Vec<bool>> = std::collections::HashMap::new();
    let mut hits = BinaryHeap::<RankedHit>::with_capacity(take.min(10_000));
    let mut matched = 0u64;
    while hits.len() < take {
        let Some(row) = rows.next().map_err(sql_error)? else {
            break;
        };
        let id: i64 = row.get(0).map_err(sql_error)?;
        if let Some(bytes) = row.get::<_, Option<Vec<u8>>>(1).map_err(sql_error)? {
            let record: FileRecord = bincode::deserialize(&bytes).map_err(io::Error::other)?;
            let shadowed = delta
                .map(|delta| delta.shadows_batch(&[record.path.as_ref()]))
                .transpose()?
                .and_then(|v| v.first().copied())
                .unwrap_or(false);
            if !shadowed && query.passes_filters(&record) {
                if let Some(score) = matcher.score(&record) {
                    matched += 1;
                    push_best(
                        &mut hits,
                        RankedHit {
                            sort: query.sort,
                            score,
                            record,
                        },
                        take,
                    );
                }
            }
            continue;
        }
        let base = &browser.base;
        if id <= 0 || id as u64 > base.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid browse record address",
            ));
        }
        let address = (id - 1) as usize;
        let block = (address / crate::compact::BLOCK_RECORDS) as u32;
        if !decoded.contains_key(&block) {
            if decoded.len() >= 64 {
                decoded.clear();
            }
            decoded.insert(block, base.read_block(block)?);
            base.release_block(block);
            let records = &decoded[&block];
            shadows.insert(
                block,
                match delta {
                    Some(delta) => delta.shadows_batch(
                        &records.iter().map(|r| r.path.as_ref()).collect::<Vec<_>>(),
                    )?,
                    None => vec![false; records.len()],
                },
            );
        }
        let record = decoded[&block]
            .get(address % crate::compact::BLOCK_RECORDS)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid browse record slot")
            })?;
        let shadowed = shadows
            .get(&block)
            .and_then(|v| v.get(address % crate::compact::BLOCK_RECORDS))
            .copied()
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "missing browse shadow block")
            })?;
        if shadowed || !query.passes_filters(record) {
            continue;
        }
        if let Some(score) = matcher.score(record) {
            matched += 1;
            push_best(
                &mut hits,
                RankedHit {
                    sort: query.sort,
                    record: record.clone(),
                    score,
                },
                take,
            );
        }
    }
    if let Some(delta) = delta {
        delta.for_each_upsert(|record| {
            if !after.is_some_and(|anchor| {
                compare_records(query.sort, &(0, &record), &(0, anchor))
                    != std::cmp::Ordering::Greater
            }) && query.passes_filters(&record)
            {
                if let Some(score) = matcher.score(&record) {
                    matched += 1;
                    push_best(
                        &mut hits,
                        RankedHit {
                            sort: query.sort,
                            record,
                            score,
                        },
                        take,
                    );
                }
            }
            Ok(())
        })?;
    }
    let mut hits = hits.into_vec();
    hits.sort_unstable_by(|a, b| {
        compare_records(query.sort, &(a.score, &a.record), &(b.score, &b.record))
    });
    let hits = hits
        .into_iter()
        .skip(offset)
        .take(query.limit)
        .map(|h| SearchHit {
            record: h.record,
            score: h.score,
        })
        .collect();
    // Filtered totals are a lower bound; fetching a page never counts millions of rows.
    let unfiltered = query.kinds.is_empty()
        && query.exts.is_empty()
        && query.fss.is_empty()
        && query.min_size.is_none()
        && query.max_size.is_none()
        && query.under.is_none()
        && (query.scope_roots.is_empty() || query.scope_roots == ["/"])
        && !query.executable_only
        && query.exclude_roots.is_empty();
    let stats = SearchStats {
        scanned: base.len(),
        matched: if unfiltered { base.len() } else { matched },
        wall_us: started.elapsed().as_micros() as u64,
    };
    Ok((hits, stats))
}

struct RankedHit {
    sort: SortKey,
    score: u32,
    record: FileRecord,
}
impl PartialEq for RankedHit {
    fn eq(&self, o: &Self) -> bool {
        compare_records(
            self.sort,
            &(self.score, &self.record),
            &(o.score, &o.record),
        ) == Ordering::Equal
    }
}
impl Eq for RankedHit {}
impl PartialOrd for RankedHit {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for RankedHit {
    fn cmp(&self, o: &Self) -> Ordering {
        compare_records(
            self.sort,
            &(self.score, &self.record),
            &(o.score, &o.record),
        )
    }
}
fn push_best(heap: &mut BinaryHeap<RankedHit>, hit: RankedHit, limit: usize) {
    if limit == 0 {
        return;
    }
    if heap.len() < limit {
        heap.push(hit);
    } else if heap.peek().is_some_and(|worst| hit < *worst) {
        heap.pop();
        heap.push(hit);
    }
}

fn ordering(sort: SortKey) -> (&'static str, &'static str) {
    match sort {
        SortKey::NameAsc => ("name COLLATE NOCASE ASC,path ASC", "by_name"),
        SortKey::NameDesc => ("name COLLATE NOCASE DESC,path DESC", "by_name"),
        SortKey::PathAsc => ("path ASC", "by_path"),
        SortKey::PathDesc => ("path DESC", "by_path"),
        SortKey::SizeAsc => ("bytes ASC", "by_size"),
        SortKey::SizeDesc => ("bytes DESC", "by_size"),
        SortKey::MtimeAsc => ("mtime ASC", "by_modified"),
        SortKey::MtimeDesc | SortKey::Relevance => ("mtime DESC", "by_modified"),
    }
}

fn tie_path_direction(sort: SortKey) -> &'static str {
    match sort {
        SortKey::NameDesc
        | SortKey::PathDesc
        | SortKey::SizeDesc
        | SortKey::MtimeDesc
        | SortKey::Relevance => "DESC",
        _ => "ASC",
    }
}

fn cursor_clause(sort: SortKey, record: &FileRecord, values: &mut Vec<Value>) -> String {
    let (key, value, operator, path_operator) = match sort {
        SortKey::NameAsc => (
            "name COLLATE NOCASE",
            Value::Text(record.name().to_owned()),
            ">",
            ">",
        ),
        SortKey::NameDesc => (
            "name COLLATE NOCASE",
            Value::Text(record.name().to_owned()),
            "<",
            "<",
        ),
        SortKey::SizeAsc => (
            "bytes",
            Value::Blob(record.disk_bytes().to_be_bytes().to_vec()),
            ">",
            ">",
        ),
        SortKey::SizeDesc => (
            "bytes",
            Value::Blob(record.disk_bytes().to_be_bytes().to_vec()),
            "<",
            "<",
        ),
        SortKey::MtimeAsc => (
            "mtime",
            Value::Integer(if record.mtime > 4_102_444_800 {
                0
            } else {
                record.mtime
            }),
            ">",
            ">",
        ),
        SortKey::MtimeDesc | SortKey::Relevance => (
            "mtime",
            Value::Integer(if record.mtime > 4_102_444_800 {
                0
            } else {
                record.mtime
            }),
            "<",
            "<",
        ),
        SortKey::PathAsc | SortKey::PathDesc => {
            values.push(Value::Text(record.path.to_string()));
            return format!(
                "path {} ?",
                if sort == SortKey::PathAsc { ">" } else { "<" }
            );
        }
    };
    values.extend([value.clone(), value, Value::Text(record.path.to_string())]);
    format!("({key}{operator}? OR ({key}=? AND path{path_operator}?))")
}

fn filters(query: &Query) -> (String, Vec<Value>) {
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

fn small_scope(browser: &BrowserIndex, query: &Query) -> io::Result<bool> {
    let [root] = query.scope_roots.as_slice() else {
        return Ok(false);
    };
    if !query.scope_case_sensitive || root == "/" || !root.starts_with('/') {
        return Ok(false);
    }
    let prefix = format!("{}/", root.trim_end_matches('/'));
    let past = format!("{}0", root.trim_end_matches('/'));
    let mut statement = browser
        .db
        .prepare("SELECT 1 FROM entries INDEXED BY by_path WHERE path>=? AND path<? LIMIT 100001")
        .map_err(sql_error)?;
    let mut rows = statement.query([prefix, past]).map_err(sql_error)?;
    let mut count = 0;
    while rows.next().map_err(sql_error)?.is_some() {
        count += 1;
    }
    Ok(count <= 100_000)
}

fn scope_clause(root: &str, sensitive: bool, values: &mut Vec<Value>) -> String {
    let path = if sensitive && root.starts_with('/') {
        "path"
    } else if sensitive {
        "replace(path,char(92),'/')"
    } else {
        "lower(replace(path,char(92),'/'))"
    };
    let normalized = root.replace('\\', "/");
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
