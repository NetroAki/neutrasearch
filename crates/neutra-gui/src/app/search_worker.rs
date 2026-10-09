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

const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(180);

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
    pub(crate) fn send(&self, job: SearchJob) -> Result<(), String> {
        self.latest.store(job.id, Ordering::Release);
        self.sender.send(job).map_err(|error| error.to_string())
    }
}

pub(crate) fn spawn(events: Sender<Event>, repaint: eframe::egui::Context) -> SearchQueue {
    let (jobs, queue) = channel::<SearchJob>();
    let latest = Arc::new(AtomicU64::new(0));
    let worker_latest = latest.clone();
    std::thread::spawn(move || {
        let mut held = Held::default();
        while let Ok(mut job) = queue.recv() {
            // Keystrokes arrive faster than a full-index scan finishes, so
            // wait until typing pauses before starting one.
            while let Ok(newer) = queue.recv_timeout(DEBOUNCE) {
                job = newer;
            }
            let result = held.run(&job, &worker_latest);
            if events
                .send(Event::SearchDone { id: job.id, result })
                .is_err()
            {
                return;
            }
            repaint.request_repaint();
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
    index: Option<std::sync::Arc<CompactIndex>>,
    ranked: Option<RankedLists>,
    browser: Option<BrowserIndex>,
    delta: Option<DeltaIndex>,
    delta_stamp: Option<(std::time::SystemTime, u64)>,
}

impl Held {
    fn run(&mut self, job: &SearchJob, latest: &AtomicU64) -> SearchResult {
        if self.index.as_ref().map(|held| held.generation()) != Some(job.index.generation()) {
            self.index = Some(std::sync::Arc::clone(&job.index));
            self.ranked = None;
            self.browser = None;
            self.delta_stamp = None;
        }
        let generation = self
            .index
            .as_ref()
            .ok_or("index is unavailable")?
            .generation();
        if self.browser.is_none() {
            self.browser = BrowserIndex::open(&job.index_path, generation).ok();
        }
        if self.browser.is_none() && self.ranked.is_none() {
            self.ranked = RankedLists::open_for_compact(&job.index_path, generation).ok();
        }
        if job.query.terms.is_empty() && job.query.regex.is_none() {
            if let Some(browser) = &self.browser {
                let deadline = std::time::Instant::now() + Duration::from_secs(1);
                loop {
                    if browser
                        .covers_delta(&job.index_path)
                        .map_err(|error| error.to_string())?
                    {
                        self.delta = None;
                        self.delta_stamp = None;
                        release_scratch();
                        return browser
                            .search_after(
                                self.index.as_ref().ok_or("index is unavailable")?,
                                &job.query,
                                None,
                                job.after.as_ref(),
                            )
                            .map_err(|error| error.to_string());
                    }
                    if latest.load(Ordering::Acquire) != job.id {
                        return Err("search superseded".into());
                    }
                    if std::time::Instant::now() >= deadline {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
            }
        }
        let loaded = self.ranked.is_some() && self.delta_stamp.is_none();
        self.refresh_delta(&job.index_path, generation)?;
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
        let mut query = job.query.clone();
        query.limit = query.limit.saturating_add(job.offset);
        index
            .search_interruptible(&query, self.delta.as_ref(), &|| {
                latest.load(Ordering::Acquire) != job.id
            })
            .map(|(hits, stats)| (hits.into_iter().skip(job.offset).collect(), stats))
            .map_err(|e| e.to_string())
    }

    /// Re-read the live delta only when the file changed, so a saved file
    /// shows up on the next search without reopening it every time.
    fn refresh_delta(
        &mut self,
        index_path: &std::path::Path,
        generation: u64,
    ) -> Result<(), String> {
        let path = index_path.with_extension("delta");
        let stamp = match std::fs::metadata(&path) {
            Ok(meta) => Some((
                meta.modified().map_err(|error| error.to_string())?,
                meta.len(),
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(format!(
                    "Cannot read live index {}: {error}",
                    path.display()
                ))
            }
        };
        if stamp == self.delta_stamp && (stamp.is_none() == self.delta.is_none()) {
            return Ok(());
        }
        let refreshed = self
            .delta
            .as_mut()
            .filter(|delta| delta.generation() == generation)
            .is_some_and(|delta| delta.refresh().is_ok());
        if !refreshed {
            self.delta =
                match stamp {
                    Some(_) => Some(DeltaIndex::open_snapshot(&path, generation).map_err(
                        |error| format!("Cannot load live index {}: {error}", path.display()),
                    )?),
                    None => None,
                };
        }
        self.delta_stamp = stamp;
        Ok(())
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
