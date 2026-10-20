use super::*;

impl DirectorySummaryOverlay {
    pub(super) fn base_removed(&self, path: &str) -> bool {
        let path = canonical_path(path);
        self.removed_prefixes
            .iter()
            .any(|prefix| crate::paths::path_in_portable_root(&path, prefix))
    }

    pub(super) fn remove_prefix(&mut self, path: &str, previous: &[FileRecord]) -> io::Result<()> {
        let path = normalize_path(path)?;
        let canonical = canonical_path(&path);
        let mut sources: HashSet<_> = self
            .base
            .entries()
            .iter()
            .filter(|entry| canonical_path(&entry.path) == canonical)
            .map(|entry| entry.source)
            .collect();
        sources.extend(
            self.live_directories
                .iter()
                .filter(|key| key.canonical == canonical)
                .map(|key| key.source),
        );
        for source in sources {
            if let Some(entry) = self.aggregate_entry(source, &path) {
                let counted_root = self.live_directories.contains(&entry_key(source, &path))
                    || previous
                        .iter()
                        .any(|record| record.source == source && record.kind == FileKind::Dir);
                self.subtract_summary(source, &path, &entry, counted_root);
            }
            self.child_changes
                .entry(entry_key(source, &parent_path(&path)))
                .or_default()
                .insert(canonical.clone(), None);
        }
        for record in previous
            .iter()
            .filter(|record| record.kind != FileKind::Dir)
        {
            self.adjust_record(record, -1)?;
            self.set_child(record, None)?;
        }
        self.adjustments
            .retain(|key, _| !crate::paths::path_in_portable_root(&key.canonical, &canonical));
        self.child_changes
            .retain(|key, _| !crate::paths::path_in_portable_root(&key.canonical, &canonical));
        self.live_directories
            .retain(|key| !crate::paths::path_in_portable_root(&key.canonical, &canonical));
        self.removed_prefixes
            .retain(|prefix| !crate::paths::path_in_portable_root(prefix, &canonical));
        if !self.base_removed(&path) {
            self.removed_prefixes.push(canonical);
        }
        Ok(())
    }

    fn subtract_summary(
        &mut self,
        source: u32,
        path: &str,
        summary: &DirectorySummaryEntry,
        counted_root: bool,
    ) {
        let ancestors = ancestor_paths(path);
        for ancestor in ancestors.iter().take(ancestors.len().saturating_sub(1)) {
            let delta = self
                .adjustments
                .entry(entry_key(source, ancestor))
                .or_default();
            delta.logical_bytes -= i128::from(summary.logical_bytes);
            delta.physical_bytes -= i128::from(summary.physical_bytes);
            delta.file_count -= i128::from(summary.file_count);
            delta.directory_count -= i128::from(summary.directory_count) + i128::from(counted_root);
        }
    }
}
