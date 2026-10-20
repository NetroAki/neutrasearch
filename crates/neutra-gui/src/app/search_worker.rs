//! Background search. The UI thread never scans the index: jobs queue here,
//! only the newest one runs, and fast typing collapses into a single search.

use super::types::Event;
use neutra_core::{
    BrowserIndex, CompactIndex, DeltaIndex, Query, RankedLists, SearchHit, SearchStats,
};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Sender};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::time::Duration;

mod catalog;
mod snapshot;

const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(45);

pub(crate) struct SearchJob {
    pub(crate) id: u64,
    pub(crate) query: Query,
    pub(crate) index_path: PathBuf,
    /// The UI's own handle, so the block tables exist in memory once.
    pub(crate) index: std::sync::Arc<CompactIndex>,
    pub(crate) offset: usize,
    pub(crate) after: Option<neutra_core::FileRecord>,
}

pub(crate) type SearchResult = Result<(Vec<SearchHit>, SearchStats), String>;

pub(crate) struct SearchQueue {
    sender: Sender<SearchJob>,
    latest: Arc<AtomicU64>,
}

impl SearchQueue {
    pub(crate) fn cancellation(&self, id: u64) -> impl Fn() -> bool + Send + Sync + 'static {
        let latest = Arc::clone(&self.latest);
        move || latest.load(Ordering::Acquire) != id
    }

    pub(crate) fn invalidate(&self, id: u64) {
        self.latest.store(id, Ordering::Release);
    }

    pub(crate) fn send(&self, job: SearchJob) -> Result<(), String> {
        self.latest.store(job.id, Ordering::Release);
        self.sender.send(job).map_err(|error| error.to_string())
    }
}

pub(crate) fn spawn(events: super::EventSender) -> SearchQueue {
    spawn_with_debounce(events, DEBOUNCE, None)
}

pub(crate) fn spawn_immediate(
    events: super::EventSender,
    index: Option<Arc<CompactIndex>>,
    path: PathBuf,
) -> SearchQueue {
    spawn_with_debounce(
        events,
        Duration::ZERO,
        index.map(|index| (path, index.generation())),
    )
}

fn spawn_with_debounce(
    events: super::EventSender,
    debounce: Duration,
    warm: Option<(PathBuf, u64)>,
) -> SearchQueue {
    let (jobs, queue) = channel::<SearchJob>();
    let latest = Arc::new(AtomicU64::new(0));
    let worker_latest = latest.clone();
    std::thread::spawn(move || {
        let mut held = Held {
            quick: debounce.is_zero(),
            ..Held::default()
        };
        if let Some((path, generation)) = warm {
            held.warm(&path, generation);
        }
        while let Ok(mut job) = queue.recv() {
            while let Ok(newer) = queue.recv_timeout(debounce) {
                job = newer;
            }
            let result = held.run(&job, &worker_latest);
            if held.quick {
                held.browser = None;
                held.delta = None;
                held.delta_stamp = None;
                release_scratch();
            }
            if events
                .send(Event::SearchDone { id: job.id, result })
                .is_err()
            {
                return;
            }
            held.warm(&job.index_path, job.index.generation());
        }
    });
    SearchQueue {
        sender: jobs,
        latest,
    }
}

/// The worker owns its own mapping, the sorted leader lists, and the latest
/// delta snapshot, so searches never borrow the UI's state.
#[derive(Default)]
struct Held {
    quick: bool,
    index: Option<std::sync::Arc<CompactIndex>>,
    ranked: Option<RankedLists>,
    browser: Option<BrowserIndex>,
    delta: Option<DeltaIndex>,
    delta_stamp: Option<(std::time::SystemTime, u64)>,
    warmed: Option<u64>,
    /// Snapshot opened off-thread by `warm`, taken by the first search that needs it.
    warm_slot: Arc<std::sync::Mutex<Option<DeltaIndex>>>,
}

impl Held {
    fn run(&mut self, job: &SearchJob, latest: &AtomicU64) -> SearchResult {
        if latest.load(Ordering::Acquire) != job.id {
            return Err("search superseded".into());
        }
        if self.index.as_ref().map(|held| held.generation()) != Some(job.index.generation()) {
            self.index = Some(std::sync::Arc::clone(&job.index));
            self.ranked = None;
            self.browser = None;
            self.delta = None;
            self.delta_stamp = None;
        }
        let generation = self
            .index
            .as_ref()
            .ok_or("index is unavailable")?
            .generation();
        if CompactIndex::generation_on_disk(&job.index_path).map_err(|error| error.to_string())?
            != generation
        {
            return Err("search index was replaced".into());
        }
        if self.browser.is_none() {
            self.browser = BrowserIndex::open(&job.index_path, generation).ok();
        }
        if self.browser.is_none() && self.ranked.is_none() {
            self.ranked = RankedLists::open_for_compact(&job.index_path, generation).ok();
        }
        if let Some(found) = self.quick_answer(job, latest, false) {
            return found;
        }
        if let Some(found) = self.cached(job, latest) {
            return found;
        }
        if let Some(found) = self.browse_ready(job, latest) {
            return found;
        }
        let loaded = self.ranked.is_some() && self.delta_stamp.is_none();
        self.refresh_delta(&job.index_path, generation)?;
        if let Some(found) = self.quick_answer(job, latest, true) {
            return found;
        }
        if loaded || self.delta_stamp.is_some() {
            release_scratch();
        }
        if let Some(browser) = &self.browser {
            if job.query.terms.is_empty() && job.query.regex.is_none() {
                return browser
                    .search_after(
                        self.index.as_ref().ok_or("index is unavailable")?,
                        &job.query,
                        self.delta.as_ref(),
                        job.after.as_ref(),
                    )
                    .map_err(|e| e.to_string());
            }
        }
        if let Some(ranked) = self.ranked.as_ref().filter(|_| job.offset == 0) {
            if let Some(found) = ranked
                .search(&job.query, self.delta.as_ref())
                .map_err(|error| error.to_string())?
            {
                return Ok(found);
            }
        }
        let index = self.index.as_ref().ok_or("index is unavailable")?;
        if let Some(browser) = self.browser.as_ref().filter(|_| !self.quick) {
            return browser
                .search_prepared(
                    &job.index_path,
                    index,
                    &job.query,
                    self.delta.as_ref(),
                    job.after.as_ref(),
                    &|| latest.load(Ordering::Acquire) != job.id,
                )
                .map_err(|error| error.to_string());
        }
        index
            .search_after_interruptible(
                &job.query,
                self.delta.as_ref(),
                job.after.as_ref(),
                &|| latest.load(Ordering::Acquire) != job.id,
            )
            .map_err(|e| e.to_string())
    }

    fn quick_answer(
        &mut self,
        job: &SearchJob,
        latest: &AtomicU64,
        overlay: bool,
    ) -> Option<SearchResult> {
        if !self.quick {
            return None;
        }
        let browser = self.browser.as_ref()?;
        let delta = if overlay { self.delta.as_mut() } else { None };
        match browser.search_quick_with_delta(&job.index_path, &job.query, delta, &|| {
            latest.load(Ordering::Acquire) != job.id
        }) {
            Ok(Some(hits)) => Some(Ok((hits, SearchStats::default()))),
            Ok(None) => None,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => None,
            Err(error) => Some(Err(error.to_string())),
        }
    }
}

/// Loading the leader lists and delta decodes megabytes of scratch; hand the
/// freed pages back so they do not count against the window.
fn release_scratch() {
    #[cfg(target_os = "linux")]
    unsafe {
        libc::malloc_trim(0);
    }
}
