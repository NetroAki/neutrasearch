//! Queries: the resident/compact search path, tree-model preparation, and
//! the small accessors the UI reads per frame.

use super::state::NeutraApp;
use super::types::Event;
 use neutra_core::{CompactIndex, FileKind, SortKey};

pub(crate) fn index_len(app: &NeutraApp) -> u64 {
    app.compact
        .as_ref()
        .map_or(app.index.len() as u64, CompactIndex::len)
}

pub(crate) fn index_is_empty(app: &NeutraApp) -> bool {
    index_len(app) == 0
}

pub(crate) fn scan_len(app: &NeutraApp) -> u64 {
    app.scan_index
        .as_ref()
        .map_or(0, |index| index.len())
}

pub(crate) fn data_generation(app: &NeutraApp) -> u64 {
    app.compact
        .as_ref()
        .map_or_else(|| app.index.generation(), CompactIndex::generation)
}

pub(crate) fn requery(app: &mut NeutraApp) {
    if app.selected_roots.is_empty() && app.onboarding_complete {
        app.hits.clear();
        app.search_stats = neutra_core::SearchStats::default();
        return;
    }
    if !refresh_compact_if_replaced(app) {
        return;
    }
    let raw_query = app.query.trim().to_owned();
    // Invalid patterns are validated here so the empty state can explain
    // itself; the engine would simply reject the query.
    let regex_valid = !app.regex_mode
        || regex::RegexBuilder::new(&raw_query)
            .case_insensitive(!app.case_sensitive)
            .build()
            .is_ok();
    if !regex_valid {
        app.hits.clear();
        app.search_stats = neutra_core::SearchStats::default();
        return;
    }
    let mut query = neutra_core::Query::parse(&raw_query);
    if app.regex_mode {
        // Regex replaces term matching entirely; the pattern is evaluated
        // by the engine (with case handling), not by re-filtering hits.
        query.terms.clear();
        query.regex = Some(raw_query.clone());
    }
    query.case_sensitive = app.case_sensitive;
    query.whole_word = app.whole_word;
    query.fold_accents = app.ignore_accents;
    query.match_fields = match app.search_mode {
        crate::ui::SearchMode::Name => neutra_core::MatchFields::Name,
        crate::ui::SearchMode::NameAndPath => neutra_core::MatchFields::NameAndPath,
        crate::ui::SearchMode::Path => neutra_core::MatchFields::Path,
    };
    query.limit = if raw_query.is_empty() {
        crate::HOME_RESULT_CAP
    } else {
        crate::TYPED_RESULT_CAP
    };
    query.sort = match (app.sort_mode, app.sort_reversed) {
        (crate::ui::SortMode::Relevance, _) => SortKey::Relevance,
        (crate::ui::SortMode::Modified, false) => SortKey::MtimeDesc,
        (crate::ui::SortMode::Modified, true) => SortKey::MtimeAsc,
        (crate::ui::SortMode::Name, false) => SortKey::NameAsc,
        (crate::ui::SortMode::Name, true) => SortKey::NameDesc,
        (crate::ui::SortMode::Size, false) => SortKey::SizeDesc,
        (crate::ui::SortMode::Size, true) => SortKey::SizeAsc,
        (crate::ui::SortMode::Path, false) => SortKey::PathAsc,
        (crate::ui::SortMode::Path, true) => SortKey::PathDesc,
    };
    apply_kind_filter(&mut query, app.kind_filter);
    let scoped_root = app
        .scope_root
        .as_deref()
        .filter(|scope| crate::scope_within_selected_roots(scope, &app.selected_roots));
    if let Some(root) = scoped_root {
        query.scope_roots.push(root.to_owned());
    } else {
        query.scope_roots.extend(
            app.selected_roots
                .iter()
                .map(|root| root.to_string_lossy().into_owned()),
        );
    }
    query.scope_case_sensitive = cfg!(not(any(target_os = "windows", target_os = "macos")));
    // Cache for result highlighting; built from the fully-assembled query.
    app.matcher = query.matcher().ok();
    if app.compact.is_some() {
        // Scanning 60M+ records takes seconds when cold, so it runs off the
        // UI thread; the previous results stay up until the new ones land.
        app.search_seq += 1;
        app.searching = true;
        let job = super::search_worker::SearchJob {
            id: app.search_seq,
            query,
            index_path: app.cache_path.clone(),
        };
        let _ = app.search_tx.send(job);
        return;
    }
    if let Ok((hits, stats)) = app.index.search(&query) {
        app.hits = hits;
        app.search_stats = stats;
    }
}

/// Reopen the compact base when a helper replaced it on disk. Returns false
/// when the current state cannot serve a query and the UI should show the
/// recorded index error instead.
/// Map the filter-row presets onto engine filters. Type presets match files
/// by extension (shared lists in neutra-core); Programs additionally matches
/// `+x` binaries without an extension via `executable_only`.
fn apply_kind_filter(query: &mut neutra_core::Query, filter: crate::ui::KindFilter) {
    use crate::ui::KindFilter as Preset;
    let files = || vec![FileKind::File, FileKind::Symlink];
    let exts = |list: &[&str]| list.iter().map(|ext| ext.to_string()).collect();
    match filter {
        Preset::All => {}
        Preset::Files => query.kinds = files(),
        Preset::Folders => query.kinds = vec![FileKind::Dir],
        Preset::Audio => {
            query.kinds = files();
            query.exts = exts(neutra_core::AUDIO_EXTS);
        }
        Preset::Images => {
            query.kinds = files();
            query.exts = exts(neutra_core::IMAGE_EXTS);
        }
        Preset::Video => {
            query.kinds = files();
            query.exts = exts(neutra_core::VIDEO_EXTS);
        }
        Preset::Programs => {
            query.kinds = files();
            query.executable_only = true;
        }
        Preset::Compressed => {
            query.kinds = files();
            query.exts = exts(neutra_core::ARCHIVE_EXTS);
        }
        Preset::Documents => {
            query.kinds = files();
            query.exts = exts(neutra_core::DOC_EXTS);
        }
    }
}

fn refresh_compact_if_replaced(app: &mut NeutraApp) -> bool {
    let Some(current_generation) = app.compact.as_ref().map(CompactIndex::generation) else {
        return true;
    };
    match CompactIndex::generation_on_disk(&app.cache_path) {
        Ok(generation) if generation == current_generation => true,
        Ok(_) => match CompactIndex::open_fast(&app.cache_path) {
            Ok(compact) => {
                app.compact = Some(compact);
                app.tree_model = None;
                true
            }
            Err(error) => {
                reject_index(app, format!("replacement rejected: {error}"));
                false
            }
        },
        Err(error) => {
            reject_index(app, format!("unavailable: {error}"));
            false
        }
    }
}

fn reject_index(app: &mut NeutraApp, status: String) {
    app.hits.clear();
    app.lanes.insert(
        "index".into(),
        super::types::LaneState {
            label: "Durable index".into(),
            status,
            error: true,
            ..super::types::LaneState::default()
        },
    );
}

 /// Fetch whatever the visible set lacks: root, the current chain, expanded
 /// branches. Each fetch streams its subtrees with pages released, so tree
 /// browsing holds megabytes, not the tens of gigabytes of full builds.
 pub(crate) fn request_tree_model(app: &mut NeutraApp) {
     if index_is_empty(app) || app.tree_summary_pending {
         return;
     }
     let missing: Vec<String> = crate::ui::Hierarchy::missing_dirs(
         app.tree_model.as_ref(),
         &app.treemap_path,
         &app.tree_expanded,
     )
     .into_iter()
     .filter(|path| !app.tree_pending.contains(path))
     .collect();
     if missing.is_empty() {
         return;
     }
     app.tree_building = true;
     app.tree_pending.extend(missing.iter().cloned());
     let generation = data_generation(app);
     let tx = app.tx.clone();
     let current = app.treemap_path.clone();
     let compact_path = app.compact.as_ref().map(|_| app.cache_path.clone());
     let resident_records = compact_path
         .is_none()
         .then(|| app.index.records().to_vec());
     std::thread::spawn(move || {
         let model = fetch_tree_dirs(&missing, &current, compact_path, generation, resident_records);
         match model {
             Ok(model) => {
                 let _ = tx.send(Event::TreeReady { generation, model });
             }
             Err(error) => {
                 let _ = tx.send(Event::TreeFailed(error));
             }
         }
     });
 }

/// Build the shallow folder summary in the background so the tree opens
/// instantly. The tree waits for it instead of scanning the whole index.
pub(crate) fn ensure_tree_summary(app: &mut NeutraApp) {
    let Some(compact) = &app.compact else { return };
    if app.tree_summary_pending {
        return;
    }
    let generation = compact.generation();
    let path = app.cache_path.clone();
    let tx = app.tx.clone();
    app.tree_summary_pending = true;
    app.tree_building = true;
    std::thread::spawn(move || {
        let ok = neutra_core::TreeSummary::ensure(&path, generation).is_ok();
        let _ = tx.send(Event::TreeSummary { generation, ok });
    });
}

 fn fetch_tree_dirs(
    missing: &[String],
    current: &str,
    compact_path: Option<std::path::PathBuf>,
    generation: u64,
    resident_records: Option<Vec<neutra_core::FileRecord>>,
) -> Result<crate::ui::Hierarchy, String> {
    let Some(path) = compact_path else {
        return Ok(crate::ui::Hierarchy::from_records(
            &resident_records.unwrap_or_default(),
        ));
    };
    // Folders within the summary's depth answer instantly; deeper ones
    // stream just their own subtree from the index.
    let summary = neutra_core::TreeSummary::open_for_compact(&path, generation).ok();
    let mut opened: Option<(CompactIndex, Option<neutra_core::DeltaIndex>)> = None;
    let mut partial = crate::ui::Hierarchy::empty();
    for dir in missing {
        let listing = match summary.as_ref().and_then(|tree| tree.listing(dir)) {
            Some(listing) => listing,
            None => {
                if opened.is_none() {
                    let pair = CompactIndex::open_with_delta_snapshot_fast(&path)
                        .map_err(|error| format!("cannot open index for disk hierarchy: {error}"))?;
                    if pair.0.generation() != generation {
                        return Err("index replaced mid-fetch".into());
                    }
                    opened = Some(pair);
                }
                let (compact, delta) = opened.as_ref().expect("opened above");
                compact
                    .list_directory(dir, None, delta.as_ref())
                    .map_err(|error| format!("cannot list {dir}: {error}"))?
            }
        };
        let folder = crate::ui::Hierarchy::folder_from_listing(dir, listing);
        partial.insert(dir.clone(), folder, dir == current);
    }
    Ok(partial)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_presets_map_to_engine_filters() {
        let mut query = neutra_core::Query::default();
        apply_kind_filter(&mut query, crate::ui::KindFilter::Audio);
        assert!(query.kinds.contains(&neutra_core::FileKind::File));
        assert!(query.exts.iter().any(|ext| ext == "mp3"));
        assert!(query.exts.iter().any(|ext| ext == "wav"));
        let mut programs = neutra_core::Query::default();
        apply_kind_filter(&mut programs, crate::ui::KindFilter::Programs);
        assert!(programs.executable_only);
        assert!(programs.exts.is_empty());
        let mut folders = neutra_core::Query::default();
        apply_kind_filter(&mut folders, crate::ui::KindFilter::Folders);
        assert_eq!(folders.kinds, vec![neutra_core::FileKind::Dir]);
    }
}
