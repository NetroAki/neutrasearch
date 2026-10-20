use super::{Arc, DeltaIndex, Held};

impl Held {
    /// Open the live-change snapshot before typing; indexed recall needs no
    /// full base-file read and leaves unrelated index pages out of RAM.
    pub(super) fn warm(&mut self, index_path: &std::path::Path, generation: u64) {
        if self.quick {
            return;
        }
        if self.warmed == Some(generation) {
            return;
        }
        self.warmed = Some(generation);
        let path = index_path.to_path_buf();
        let slot = Arc::clone(&self.warm_slot);
        std::thread::spawn(move || {
            if let Ok(delta) = DeltaIndex::open_snapshot(&path.with_extension("delta"), generation)
            {
                if let Ok(mut held) = slot.lock() {
                    *held = Some(delta);
                }
            }
        });
    }

    /// Refresh only when the WAL changes, preserving a private coherent view.
    pub(super) fn refresh_delta(
        &mut self,
        index_path: &std::path::Path,
        generation: u64,
    ) -> Result<(), String> {
        let path = index_path.with_extension("delta");
        if self.quick {
            self.delta = match std::fs::metadata(&path) {
                Ok(_) => Some(
                    DeltaIndex::open_search_snapshot(&path, generation)
                        .or_else(|_| DeltaIndex::open_snapshot(&path, generation))
                        .map_err(|error| {
                            format!("Cannot load live index {}: {error}", path.display())
                        })?,
                ),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.to_string()),
            };
            return Ok(());
        }
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
            let warmed = self
                .warm_slot
                .lock()
                .ok()
                .and_then(|mut slot| slot.take())
                .filter(|delta| delta.generation() == generation && stamp.is_some());
            self.delta = match (warmed, stamp) {
                (Some(mut delta), _) => delta.refresh().is_ok().then_some(delta),
                _ => None,
            };
            if self.delta.is_some() {
                self.delta_stamp = stamp;
                return Ok(());
            }
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
