use crate::compact::compare_index_paths;
use crate::dir_summary::compare_paths;
use crate::FileRecord;

use super::SpillOrder;

/// Merge key with a deterministic run-index tiebreak. Ordered ascending so
/// `Reverse` turns the max-heap into a min-merge. Keys order exactly as the
/// run sort does (`compare_index_paths` / `compare_paths`), so folded paths
/// from different runs interleave the same way the sorted runs do.
pub(super) struct MergeKey {
    key: MergeKeyInner,
    pub(super) run: usize,
}

enum MergeKeyInner {
    Compact(Box<str>),
    Summary(u32, Box<str>),
}

impl Ord for MergeKeyInner {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match (self, other) {
            (Self::Compact(left), Self::Compact(right)) => compare_index_paths(left, right),
            (Self::Summary(ls, left), Self::Summary(rs, right)) => {
                ls.cmp(rs).then_with(|| compare_paths(left, right))
            }
            (Self::Compact(_), Self::Summary(..)) => std::cmp::Ordering::Less,
            (Self::Summary(..), Self::Compact(_)) => std::cmp::Ordering::Greater,
        }
    }
}

impl PartialOrd for MergeKeyInner {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for MergeKeyInner {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}

impl Eq for MergeKeyInner {}

impl Ord for MergeKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.key
            .cmp(&other.key)
            .then_with(|| self.run.cmp(&other.run))
    }
}

impl PartialOrd for MergeKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for MergeKey {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}

impl Eq for MergeKey {}

pub(super) fn key_for(order: SpillOrder, record: &FileRecord, run: usize) -> MergeKey {
    let key = match order {
        SpillOrder::Compact => MergeKeyInner::Compact(record.path.clone()),
        SpillOrder::Summary => MergeKeyInner::Summary(record.source, record.path.clone()),
    };
    MergeKey { key, run }
}

pub(super) fn compare(
    order: SpillOrder,
    left: &FileRecord,
    right: &FileRecord,
) -> std::cmp::Ordering {
    match order {
        SpillOrder::Compact => compare_index_paths(left.path.as_ref(), right.path.as_ref()),
        SpillOrder::Summary => left
            .source
            .cmp(&right.source)
            .then_with(|| compare_paths(left.path.as_ref(), right.path.as_ref())),
    }
}
