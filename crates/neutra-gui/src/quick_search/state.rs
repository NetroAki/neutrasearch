use crate::app::search_worker::SearchResult;
use neutra_core::SearchHit;

#[derive(Default)]
pub(super) struct SearchState {
    pub query: String,
    pub hits: Vec<SearchHit>,
    pub error: Option<String>,
    pub selected: usize,
    pub sequence: u64,
    pub pending: bool,
}

impl SearchState {
    pub fn begin(&mut self) -> u64 {
        self.sequence += 1;
        self.hits.clear();
        self.selected = 0;
        self.error = None;
        self.pending = !self.query.trim().is_empty();
        self.sequence
    }

    pub fn complete(&mut self, id: u64, result: SearchResult) {
        if id != self.sequence || !self.pending {
            return;
        }
        self.pending = false;
        match result {
            Ok((hits, _)) => {
                self.hits = hits;
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }

    pub fn selected_hit(&self) -> Option<&SearchHit> {
        if self.pending {
            return None;
        }
        self.hits.get(self.selected)
    }
}
