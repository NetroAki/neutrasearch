use super::*;

impl DeltaIndex {
    pub(crate) fn scan_upserts<A: Send>(
        &self,
        parts: usize,
        init: impl Fn() -> A + Sync,
        visit: impl Fn(&mut A, &crate::record_ref::RecordRef<'_>) -> io::Result<()> + Sync,
    ) -> io::Result<Vec<A>> {
        use rayon::prelude::*;
        let mut overlay = self
            .overlay
            .lock()
            .map_err(|_| io::Error::other("delta overlay lock poisoned"))?;
        if overlay.is_shared() {
            // Separate connections would observe different commits. Use the
            // held transaction for both shadow checks and live records.
            let mut state = init();
            overlay.scan_shared(|view| visit(&mut state, view))?;
            return Ok(vec![state]);
        }
        let (db_path, ranges) = overlay.upsert_ranges(parts)?;
        drop(overlay);
        ranges
            .par_iter()
            .map(|range| {
                let mut state = init();
                storage::scan_range(&db_path, range, |view| visit(&mut state, view))?;
                Ok(state)
            })
            .collect()
    }
}
