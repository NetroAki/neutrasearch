use super::state::SearchState;
use neutra_core::{FileKind, FileRecord, FsKind, SearchHit, SearchStats};

fn populated() -> SearchState {
    let hits = ["/music/voice.wav", "/music/guitar.wav"]
        .into_iter()
        .map(|path| SearchHit {
            score: 1,
            record: FileRecord {
                path: path.into(),
                size: 1,
                disk: 1,
                mtime: 0,
                mode: 0,
                kind: FileKind::File,
                fs: FsKind::Btrfs,
                native_id: 0,
                native_parent: 0,
                source: 0,
            },
        })
        .collect();
    SearchState {
        query: "voice".into(),
        hits,
        ..Default::default()
    }
}

#[test]
fn typing_clears_previous_results_and_prevents_opening_them() {
    let mut state = populated();
    state.selected = 1;
    state.error = Some("old failure".into());
    assert!(state.selected_hit().is_some());
    state.query = "different".into();
    state.begin();
    assert!(state.pending);
    assert!(state.hits.is_empty());
    assert_eq!(state.selected, 0);
    assert!(state.error.is_none());
    assert!(state.selected_hit().is_none());
}

#[test]
fn superseded_completions_cannot_finish_or_replace_the_current_search() {
    let mut state = populated();
    let old_hits = state.hits.clone();
    let old_id = state.begin();
    let current_id = state.begin();
    state.complete(old_id, Ok((old_hits.clone(), SearchStats::default())));
    state.complete(old_id, Err("superseded failure".into()));
    assert!(state.pending);
    assert!(state.hits.is_empty());
    assert!(state.error.is_none());
    state.complete(current_id, Ok((old_hits, SearchStats::default())));
    assert!(!state.pending);
    assert!(state.selected_hit().is_some());
    state.complete(old_id, Err("late failure".into()));
    assert!(state.error.is_none());
    assert!(state.selected_hit().is_some());
}

#[test]
fn clearing_query_and_empty_or_failed_searches_leave_no_stale_hits() {
    let mut state = populated();
    state.query.clear();
    let id = state.begin();
    assert!(state.hits.is_empty());
    assert!(!state.pending);
    state.complete(id, Ok((Vec::new(), SearchStats::default())));
    assert!(!state.pending);
    assert!(state.selected_hit().is_none());
    state.query = "voice".into();
    let id = state.begin();
    state.complete(id, Err("index unavailable".into()));
    assert!(!state.pending);
    assert_eq!(state.error.as_deref(), Some("index unavailable"));
    assert!(state.selected_hit().is_none());
    state.begin();
    assert!(state.error.is_none());
}

#[test]
fn clearing_the_field_cancels_in_flight_results_and_returns_to_idle() {
    let mut state = populated();
    let old_hits = state.hits.clone();
    let old = state.begin();
    state.query = "  ".into();
    state.begin();
    state.complete(old, Ok((old_hits, SearchStats::default())));
    assert!(!state.pending);
    assert!(state.hits.is_empty());
    assert!(state.error.is_none());
}

#[test]
fn refocus_select_all_covers_the_existing_query() {
    // Same native mechanism search_field uses on repeat-shortcut refocus:
    // select the whole query through the TextEdit state so typing replaces
    // it instead of appending. Runs headless via egui's test UI.
    egui::__run_test_ui(|ui| {
        let mut query = String::from("voice");
        let output = egui::TextEdit::singleline(&mut query).show(ui);
        let mut edit_state = output.state.clone();
        edit_state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::select_all(&output.galley)));
        edit_state.store(ui.ctx(), output.response.id);
        let stored = egui::text_edit::TextEditState::load(ui.ctx(), output.response.id)
            .expect("stored query selection");
        let range = stored.cursor.char_range().expect("query selection");
        assert_eq!(range.as_sorted_char_range(), 0..query.chars().count());
    });
}
