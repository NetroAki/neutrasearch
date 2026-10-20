use super::*;

pub(super) fn run(index: &mut Store, allowed_roots: &[PathBuf], args: Value) -> Value {
    let raw = args.get("query").and_then(Value::as_str).unwrap_or("");
    let mut q = Query::parse(raw);
    q.limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(50)
        .clamp(1, 1000) as usize;
    q.scope_roots = allowed_roots
        .iter()
        .map(|root| root.to_string_lossy().into_owned())
        .collect();
    q.scope_case_sensitive = cfg!(not(any(target_os = "windows", target_os = "macos")));
    let metadata = args
        .get("metadata")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let (hits, stats) = match index.search(&q) {
        Ok(result) => result,
        Err(error) => {
            return json!({"isError":true,"content":[{"type":"text","text":format!("index search failed: {error}")}]})
        }
    };
    // Defense in depth: trusted scopes are already applied by the query
    // engine before ranking and limiting.
    let hits = hits
        .into_iter()
        .filter(|hit| path_is_allowed(Path::new(hit.record.path.as_ref()), allowed_roots))
        .collect::<Vec<_>>();
    let returned = hits.len();
    let paths = hits
        .iter()
        .map(|h| h.record.path.to_string())
        .collect::<Vec<_>>();
    let text = if metadata {
        hits.iter()
            .map(|h| {
                format!(
                    "{}\t{:?}\t{}\t{}\t{}",
                    h.record.path,
                    h.record.kind,
                    h.record.size,
                    h.record.mtime,
                    h.record.fs.label()
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        paths.join("\n")
    };
    let header = format!(
        "# matched={} returned={} search_us={}",
        stats.matched, returned, stats.wall_us
    );
    json!({"content":[{"type":"text","text":if text.is_empty(){header}else{format!("{header}\n{text}")}}],"structuredContent":{"paths":paths,"matched":stats.matched,"returned":returned,"search_us":stats.wall_us}})
}
