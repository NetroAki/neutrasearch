use neutra_core::DeltaChange;
use std::collections::{BTreeMap, VecDeque};

pub(super) struct Pending {
    prefixes: VecDeque<Box<str>>,
    leaves: BTreeMap<Box<str>, DeltaChange>,
    pub(super) retry: bool,
}

pub(super) fn path(change: &DeltaChange) -> &str {
    match change {
        DeltaChange::Upsert(record) => &record.path,
        DeltaChange::Remove(path) | DeltaChange::RemovePrefix(path) => path,
    }
}

impl Pending {
    pub(super) fn new(changes: Vec<DeltaChange>) -> Self {
        let mut pending = Self {
            prefixes: VecDeque::new(),
            leaves: BTreeMap::new(),
            retry: false,
        };
        for change in changes {
            if let DeltaChange::RemovePrefix(root) = change {
                pending.prefixes.push_back(root);
            } else {
                pending.leaves.insert(path(&change).into(), change);
            }
        }
        pending
    }

    pub(super) fn supersede(&mut self, newer: DeltaChange) {
        let current = path(&newer);
        let subtree = matches!(newer, DeltaChange::RemovePrefix(_));
        let mut conflicts = Vec::new();
        self.prefixes.retain(|root| {
            let conflict = neutra_core::paths::path_in_portable_root(current, root)
                || (subtree && neutra_core::paths::path_in_portable_root(root, current));
            if conflict {
                conflicts.push(root.clone());
            }
            !conflict
        });
        for root in conflicts {
            // Replan the whole removed folder; dropping only its tombstone
            // would leave old children behind when the native cursor advances.
            self.remove_subtree(&root);
            self.retry = true;
        }
        if subtree {
            self.remove_subtree(current);
        } else {
            self.leaves.remove(current);
        }
    }

    fn remove_subtree(&mut self, root: &str) {
        self.leaves.remove(root);
        let root = root.trim_end_matches('/');
        let mut selected = self.leaves.split_off(format!("{root}/").as_str());
        let mut after = selected.split_off(format!("{root}0").as_str());
        self.leaves.append(&mut after);
    }

    pub(super) fn is_empty(&self) -> bool {
        self.prefixes.is_empty() && self.leaves.is_empty()
    }

    pub(super) fn batch(&mut self, limit: usize) -> Vec<DeltaChange> {
        let mut batch = Vec::with_capacity(limit);
        while batch.len() < limit {
            if let Some(root) = self.prefixes.pop_front() {
                batch.push(DeltaChange::RemovePrefix(root));
            } else if let Some((_, change)) = self.leaves.pop_first() {
                batch.push(change);
            } else {
                break;
            }
        }
        batch
    }
}

#[cfg(test)]
mod tests;
