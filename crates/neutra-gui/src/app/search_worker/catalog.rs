use super::{release_scratch, AtomicU64, Duration, Held, Ordering, SearchJob, SearchResult};

impl Held {
    pub(super) fn cached(&mut self, job: &SearchJob, latest: &AtomicU64) -> Option<SearchResult> {
        let browser = self.browser.as_ref()?;
        if !self.quick
            && (browser.has_prepared_search(&job.query).ok()?
                || browser.has_short_search(&job.query).ok()?)
        {
            let deadline = std::time::Instant::now() + Duration::from_secs(1);
            while !browser.covers_delta(&job.index_path).ok()? {
                if latest.load(Ordering::Acquire) != job.id {
                    return Some(Err("search superseded".into()));
                }
                if std::time::Instant::now() >= deadline {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
        let found = browser
            .search_cached(&job.index_path, &job.query, job.after.as_ref())
            .and_then(|cached| {
                if cached.is_some() {
                    return Ok(cached);
                }
                browser.search_short(&job.index_path, &job.query, job.after.as_ref(), &|| {
                    latest.load(Ordering::Acquire) != job.id
                })
            });
        match found {
            Ok(Some(found)) => {
                release_scratch();
                Some(Ok(found))
            }
            Ok(None) => None,
            Err(error) => {
                eprintln!("Prepared search unavailable: {error}");
                None
            }
        }
    }

    pub(super) fn browse_ready(
        &mut self,
        job: &SearchJob,
        latest: &AtomicU64,
    ) -> Option<SearchResult> {
        if !job.query.terms.is_empty() || job.query.regex.is_some() {
            return None;
        }
        let browser = self.browser.as_ref()?;
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        loop {
            match browser.covers_delta(&job.index_path) {
                Ok(true) => {
                    release_scratch();
                    let index = self.index.as_ref()?;
                    return Some(
                        browser
                            .search_after(index, &job.query, None, job.after.as_ref())
                            .map_err(|error| error.to_string()),
                    );
                }
                Ok(false) => {}
                Err(error) => return Some(Err(error.to_string())),
            }
            if latest.load(Ordering::Acquire) != job.id {
                return Some(Err("search superseded".into()));
            }
            if std::time::Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}
