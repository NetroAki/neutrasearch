//! Background search. The UI thread never scans the index: jobs queue here,
//! only the newest one runs, and fast typing collapses into a single search.

use super::types::Event;
use neutra_core::{CompactIndex, DeltaIndex, Query, RankedLists, SearchHit, SearchStats};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Sender};

const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(180);

pub(crate) struct SearchJob {
    pub(crate) id: u64,
    pub(crate) query: Query,
    pub(crate) index_path: PathBuf,
}

pub(crate) type SearchResult = Option<(Vec<SearchHit>, SearchStats)>;

pub(crate) fn spawn(events: Sender<Event>, repaint: eframe::egui::Context) -> Sender<SearchJob> {
    let (jobs, queue) = channel::<SearchJob>();
    std::thread::spawn(move || {
        let mut held = Held::default();
        while let Ok(mut job) = queue.recv() {
            // Keystrokes arrive faster than a full-index scan finishes, so
            // wait until typing pauses before starting one.
            while let Ok(newer) = queue.recv_timeout(DEBOUNCE) {
                job = newer;
            }
            let result = held.run(&job);
            if events.send(Event::SearchDone { id: job.id, result }).is_err() {
                return;
            }
            repaint.request_repaint();
        }
    });
    jobs
}

/// The worker owns its own mapping, the sorted leader lists, and the latest
/// delta snapshot, so searches never borrow the UI's state.
#[derive(Default)]
struct Held {
    index: Option<CompactIndex>,
    ranked: Option<RankedLists>,
    delta: Option<DeltaIndex>,
    delta_stamp: Option<(std::time::SystemTime, u64)>,
}

impl Held {
    fn run(&mut self, job: &SearchJob) -> SearchResult {
        let on_disk = CompactIndex::generation_on_disk(&job.index_path).ok()?;
        if self.index.as_ref().map(CompactIndex::generation) != Some(on_disk) {
            self.index = CompactIndex::open_fast(&job.index_path).ok();
            self.ranked = None;
            self.delta_stamp = None;
        }
        let generation = self.index.as_ref()?.generation();
        if self.ranked.is_none() {
            self.ranked = RankedLists::open_for_compact(&job.index_path, generation).ok();
        }
        self.refresh_delta(&job.index_path, generation);
        if let Some(found) = self.ranked.as_ref().and_then(|r| r.search(&job.query, self.delta.as_ref())) {
            return Some(found);
        }
        let index = self.index.as_ref()?;
        match &self.delta {
            Some(delta) => index.search_with_delta(&job.query, delta).ok(),
            None => index.search(&job.query).ok(),
        }
    }

    /// Re-read the live delta only when the file changed, so a saved file
    /// shows up on the next search without reopening it every time.
    fn refresh_delta(&mut self, index_path: &std::path::Path, generation: u64) {
        let path = index_path.with_extension("delta");
        let stamp = std::fs::metadata(&path)
            .ok()
            .and_then(|meta| Some((meta.modified().ok()?, meta.len())));
        if stamp == self.delta_stamp && (stamp.is_none() == self.delta.is_none()) {
            return;
        }
        self.delta = stamp.and_then(|_| DeltaIndex::open_snapshot(&path, generation).ok());
        self.delta_stamp = stamp;
    }
}
