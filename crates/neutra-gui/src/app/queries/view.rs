use super::super::state::NeutraApp;
use neutra_core::SortKey;
use neutra_core::{CompactIndex, FileKind};

/// Map filter presets to the engine's shared type and extension filters.
pub(super) fn apply_kind_filter(query: &mut neutra_core::Query, filter: crate::ui::KindFilter) {
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

/// Reopen a compact base replaced on disk, reporting errors through the index lane.
pub(super) fn refresh_compact_if_replaced(app: &mut NeutraApp) -> bool {
    let Some(current_generation) = app.compact.as_ref().map(|index| index.generation()) else {
        return true;
    };
    match CompactIndex::generation_on_disk(&app.cache_path) {
        Ok(generation) if generation == current_generation => true,
        Ok(_) => match CompactIndex::open_fast(&app.cache_path) {
            Ok(compact) => {
                app.compact = Some(std::sync::Arc::new(compact));
                app.tree_model = None;
                true
            }
            Err(error) => {
                super::reject_index(app, format!("replacement rejected: {error}"));
                false
            }
        },
        Err(error) => {
            super::reject_index(app, format!("unavailable: {error}"));
            false
        }
    }
}

/// Read missing hierarchy listings from the summary or the compact index.
pub(super) fn fetch_tree_dirs(
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
    let summary = neutra_core::TreeSummary::open_for_compact(&path, generation).ok();
    let mut opened: Option<(CompactIndex, Option<neutra_core::DeltaIndex>)> = None;
    let mut partial = crate::ui::Hierarchy::empty();
    for dir in missing {
        let listing = match summary.as_ref().and_then(|tree| tree.listing(dir)) {
            Some(listing) => listing,
            None => {
                if opened.is_none() {
                    let pair =
                        CompactIndex::open_with_delta_snapshot_fast(&path).map_err(|error| {
                            format!("cannot open index for disk hierarchy: {error}")
                        })?;
                    if pair.0.generation() != generation {
                        return Err("index replaced mid-fetch".into());
                    }
                    opened = Some(pair);
                }
                let (compact, delta) = opened.as_ref().expect("opened above");
                compact
                    .list_directory_with_limit(
                        dir,
                        None,
                        delta.as_ref(),
                        crate::ui::Hierarchy::MAX_FILE_ROWS,
                    )
                    .map_err(|error| format!("cannot list {dir}: {error}"))?
            }
        };
        let folder = crate::ui::Hierarchy::folder_from_listing(dir, listing);
        partial.insert(dir.clone(), folder, dir == current);
    }
    Ok(partial)
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
    let Some(query) = build_query(app, &raw_query) else {
        app.search_seq += 1;
        app.search_tx.invalidate(app.search_seq);
        app.searching = false;
        app.matcher = None;
        app.lanes.remove("search");
        app.hits.clear();
        app.search_stats = neutra_core::SearchStats::default();
        return;
    };
    app.matcher = query.matcher().ok();
    if let Some(compact) = &app.compact {
        app.search_seq += 1;
        app.searching = true;
        let job = super::super::search_worker::SearchJob {
            id: app.search_seq,
            query,
            index_path: app.cache_path.clone(),
            index: std::sync::Arc::clone(compact),
            offset: app.page_offset,
            after: app.page_cursors.last().cloned().flatten(),
        };
        let _ = app.search_tx.send(job);
        return;
    }
    if let Ok((hits, stats)) = app.index.search(&query) {
        app.hits = hits;
        app.search_stats = stats;
    }
}

fn build_query(app: &mut NeutraApp, raw_query: &str) -> Option<neutra_core::Query> {
    let regex_valid = !app.regex_mode
        || regex::RegexBuilder::new(raw_query)
            .case_insensitive(!app.case_sensitive)
            .build()
            .is_ok();
    if !regex_valid {
        return None;
    }
    let mut query = neutra_core::Query::parse(raw_query);
    if app.regex_mode {
        query.terms.clear();
        query.regex = Some(raw_query.to_owned());
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
        (crate::ui::SortMode::Relevance, _) if raw_query.is_empty() => SortKey::MtimeDesc,
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
    } else if !app.search_roots.is_empty() {
        query.scope_roots.extend(
            app.search_roots
                .iter()
                .map(|root| root.to_string_lossy().into_owned()),
        );
    } else {
        query.scope_roots.extend(
            app.selected_roots
                .iter()
                .map(|root| root.to_string_lossy().into_owned()),
        );
    }
    query.scope_case_sensitive = cfg!(not(any(target_os = "windows", target_os = "macos")));
    query.exclude_roots = app
        .search_excluded
        .iter()
        .map(|root| root.to_string_lossy().into_owned())
        .collect();
    if app
        .last_query
        .as_ref()
        .is_some_and(|previous| previous.scope_roots != query.scope_roots)
    {
        app.treemap_path = if query.scope_roots.len() == 1 {
            query.scope_roots[0].clone()
        } else {
            "/".into()
        };
        for ancestor in std::path::Path::new(&app.treemap_path).ancestors() {
            app.tree_expanded
                .insert(ancestor.to_string_lossy().into_owned());
        }
    }
    if app.last_query.as_ref() != Some(&query) {
        app.page_offset = 0;
        app.page_cursors = vec![None];
        app.last_query = Some(query.clone());
    }
    Some(query)
}
