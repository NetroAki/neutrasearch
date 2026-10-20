use super::{Event, NeutraApp};
use neutra_core::{FileKind, Query, SortKey};
mod worker;

#[derive(Clone, PartialEq)]
pub(crate) struct MapKey {
    generation: u64,
    query: Query,
    delta: Option<Option<(std::time::SystemTime, u64)>>,
}

pub(crate) fn request(app: &mut NeutraApp, smallest: bool) {
    if app.searching {
        app.map_file_key = None;
        app.map_files.clear();
        app.map_files_pending = false;
        return;
    }
    let Some(query) = app.last_query.as_ref().filter(|_| app.matcher.is_some()) else {
        app.map_file_key = None;
        app.map_files.clear();
        app.map_files_pending = false;
        return;
    };
    let Some(query) = scoped_query(query, &app.treemap_path, smallest) else {
        app.map_file_key = None;
        app.map_files.clear();
        app.map_files_pending = false;
        return;
    };
    let Some(index) = app.compact.clone() else {
        return;
    };
    let key = MapKey {
        generation: index.generation(),
        query,
        delta: app.delta_stamp,
    };
    if app.map_file_key.as_ref() == Some(&key) || app.map_files_pending {
        return;
    }
    app.map_file_key = Some(key.clone());
    app.map_files.clear();
    app.map_files_pending = true;
    let path = app.cache_path.clone();
    let tx = app.tx.clone();
    let cancelled = app.search_tx.cancellation(app.search_seq);
    std::thread::spawn(move || {
        let result = worker::search(&path, &index, &key.query, &cancelled)
            .map_err(|error| error.to_string());
        let _ = tx.send(Event::MapFiles { key, result });
    });
}

fn scoped_query(search: &Query, path: &str, smallest: bool) -> Option<Query> {
    let mut query = search.clone();
    if query.kinds.is_empty() {
        query.kinds = vec![FileKind::File, FileKind::Symlink];
    } else {
        query.kinds.retain(|kind| *kind != FileKind::Dir);
        if query.kinds.is_empty() {
            return None;
        }
    }
    let contains = |child: &str, root: &str| {
        crate::portable_path_in_root(child, root, query.scope_case_sensitive)
    };
    query.scope_roots = search
        .scope_roots
        .iter()
        .filter_map(|root| {
            if path == "/" {
                Some(root.clone())
            } else if contains(path, root) {
                Some(path.to_owned())
            } else if contains(root, path) {
                Some(root.clone())
            } else {
                None
            }
        })
        .collect();
    if query.scope_roots.is_empty() {
        return None;
    }
    query.sort = if smallest {
        SortKey::SizeAsc
    } else {
        SortKey::SizeDesc
    };
    query.limit = 2048;
    Some(query)
}

#[cfg(test)]
mod tests;
