use super::{Event, NeutraApp};
use neutra_core::{BrowserIndex, DeltaIndex, FileKind, Query, SortKey};

pub(crate) fn request(app: &mut NeutraApp, smallest: bool) {
    let Some(index) = app.compact.clone() else {
        return;
    };
    let key = (app.treemap_path.clone(), index.generation(), smallest);
    if app.map_file_key.as_ref() == Some(&key) {
        return;
    }
    app.map_file_key = Some(key.clone());
    app.map_files_pending = true;
    let path = app.cache_path.clone();
    let tx = app.tx.clone();
    std::thread::spawn(move || {
        let result = (|| {
            let browser = BrowserIndex::open(&path, index.generation())?;
            let delta = if browser.covers_delta(&path)? {
                None
            } else {
                match DeltaIndex::open_snapshot(&path.with_extension("delta"), index.generation()) {
                    Ok(delta) => Some(delta),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                    Err(error) => return Err(error),
                }
            };
            let query = Query {
                kinds: vec![FileKind::File, FileKind::Symlink],
                scope_roots: vec![key.0.clone()],
                scope_case_sensitive: cfg!(target_os = "linux"),
                sort: if smallest {
                    SortKey::SizeAsc
                } else {
                    SortKey::SizeDesc
                },
                limit: 2048,
                ..Query::default()
            };
            browser
                .search(&index, &query, delta.as_ref())
                .map(|(hits, _)| hits.into_iter().map(|hit| hit.record).collect())
        })()
        .map_err(|error: std::io::Error| error.to_string());
        let _ = tx.send(Event::MapFiles { key, result });
    });
}
