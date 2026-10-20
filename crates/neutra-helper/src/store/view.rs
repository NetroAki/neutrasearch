use super::*;

impl DurableStore {
    /// The base and delta as one readable pair, for in-process reconcilers.
    pub(crate) fn view(&self) -> Option<(&CompactIndex, &DeltaIndex)> {
        self.base.as_ref().map(|base| (base, &self.delta))
    }

    pub(crate) fn search(&self, query: &Query) -> Result<(Vec<SearchHit>, SearchStats)> {
        let base = self
            .base
            .as_ref()
            .context("compact base is unavailable after a failed replacement")?;
        Ok(base.search_with_delta(query, &self.delta)?)
    }

    /// One directory's live totals: the `.dirs` sidecar with the WAL overlay
    /// applied, so serve-mode clients see fresh totals between compactions.
    pub(crate) fn directory_summary(
        &self,
        source: u32,
        path: &str,
    ) -> Result<Option<DirectorySummaryEntry>> {
        let base = self
            .base
            .as_ref()
            .context("compact base is unavailable after a failed replacement")?;
        let summary = DirectorySummary::open_for_compact(&self.path, base.generation())
            .context("open directory summary sidecar")?;
        if self.delta.change_count()? == 0 {
            return Ok(summary.get(source, path).cloned());
        }
        let overlay = DirectorySummaryOverlay::from_delta(summary, base, &self.delta)?;
        Ok(overlay.get(source, path)?)
    }
}
