use super::*;

impl DirectorySummaryOverlay {
    pub fn children(&self, source: u32, path: &str) -> io::Result<Vec<DirectoryChild>> {
        let normalized = normalize_path(path)?;
        let parent_key = entry_key(source, &normalized);
        let mut children = if self.base_removed(&normalized) {
            Vec::new()
        } else {
            self.base.children(source, &normalized).unwrap_or_default()
        };
        children.retain(|child| !self.base_removed(&child.path));
        if let Some(changes) = self.child_changes.get(&parent_key) {
            apply_child_changes(&mut children, changes);
        }
        for child in &mut children {
            if child.kind == FileKind::Dir {
                if let Some(entry) = self.aggregate_entry(source, &child.path) {
                    child.logical_bytes = entry.logical_bytes;
                    child.physical_bytes = entry.physical_bytes;
                    child.file_count = entry.file_count;
                    child.directory_count = entry.directory_count;
                }
            }
        }
        children.sort_unstable_by(|left, right| {
            compare_paths(&left.path, &right.path)
                .then_with(|| left.kind_rank().cmp(&right.kind_rank()))
        });
        Ok(children)
    }
}

fn apply_child_changes(
    children: &mut Vec<DirectoryChild>,
    changes: &HashMap<String, Option<DirectoryChild>>,
) {
    let mut positions = children
        .iter()
        .enumerate()
        .map(|(index, child)| (canonical_path(&child.path), index))
        .collect::<HashMap<_, _>>();
    for (key, change) in changes {
        match change {
            Some(child) => {
                if let Some(index) = positions.get(key).copied() {
                    children[index] = child.clone();
                } else {
                    positions.insert(key.clone(), children.len());
                    children.push(child.clone());
                }
            }
            None => {
                if let Some(index) = positions.remove(key) {
                    children.swap_remove(index);
                    positions = children
                        .iter()
                        .enumerate()
                        .map(|(index, child)| (canonical_path(&child.path), index))
                        .collect();
                }
            }
        }
    }
}
