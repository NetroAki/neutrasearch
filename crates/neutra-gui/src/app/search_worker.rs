//! Background search. The UI thread never scans the index: jobs queue here,
//! only the newest one runs, and fast typing collapses into a single search.

use super::types::Event;
use neutra_core::{CompactIndex, Query, SearchHit, SearchStats};
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
        // The worker owns its own mapping so searches never borrow the UI's.
        let mut held: Option<CompactIndex> = None;
        while let Ok(mut job) = queue.recv() {
            // Keystrokes arrive faster than a full-index scan finishes, so
            // wait until typing pauses before starting one.
            while let Ok(newer) = queue.recv_timeout(DEBOUNCE) {
                job = newer;
            }
            let result = run(&mut held, &job);
            if events.send(Event::SearchDone { id: job.id, result }).is_err() {
                return;
            }
            repaint.request_repaint();
        }
    });
    jobs
}

fn run(held: &mut Option<CompactIndex>, job: &SearchJob) -> SearchResult {
    let on_disk = CompactIndex::generation_on_disk(&job.index_path).ok()?;
    if held.as_ref().map(CompactIndex::generation) != Some(on_disk) {
        *held = CompactIndex::open_fast(&job.index_path).ok();
    }
    held.as_ref()?.search(&job.query).ok()
}
