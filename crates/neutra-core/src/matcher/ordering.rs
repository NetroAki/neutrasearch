use crate::compact::compare_index_paths;
use crate::{FileRecord, SortKey};

/// ASCII case-insensitive name comparison without per-comparison allocations.
/// Used by sort comparators, which run O(n log n) times per query.
#[inline]
pub(crate) fn cmp_name_ci(left: &str, right: &str) -> std::cmp::Ordering {
    left.bytes()
        .map(|b| b.to_ascii_lowercase())
        .cmp(right.bytes().map(|b| b.to_ascii_lowercase()))
}

/// Timestamps past 2100 come from corrupt archives and would pin themselves
/// to the top of every newest-first listing, so they sort as unknown.
const LATEST_PLAUSIBLE_MTIME: i64 = 4_102_444_800;

fn sort_mtime(record: &FileRecord) -> i64 {
    if record.mtime > LATEST_PLAUSIBLE_MTIME {
        0
    } else {
        record.mtime
    }
}

/// The single sort comparator shared by the in-memory and compact engines.
/// `a`/`b` carry the relevance score plus the record; ties fall back to path
/// so ordering is deterministic.
pub(crate) fn compare_records(
    sort: SortKey,
    a: &(u32, &FileRecord),
    b: &(u32, &FileRecord),
) -> std::cmp::Ordering {
    match sort {
        SortKey::Relevance => {
            b.0.cmp(&a.0)
                .then(sort_mtime(b.1).cmp(&sort_mtime(a.1)))
                .then(compare_index_paths(&b.1.path, &a.1.path))
        }
        SortKey::NameAsc => {
            cmp_name_ci(a.1.name(), b.1.name()).then(compare_index_paths(&a.1.path, &b.1.path))
        }
        SortKey::NameDesc => {
            cmp_name_ci(b.1.name(), a.1.name()).then(compare_index_paths(&b.1.path, &a.1.path))
        }
        SortKey::PathAsc => compare_index_paths(&a.1.path, &b.1.path),
        SortKey::PathDesc => compare_index_paths(&b.1.path, &a.1.path),
        SortKey::SizeDesc => {
            b.1.disk_bytes()
                .cmp(&a.1.disk_bytes())
                .then(compare_index_paths(&b.1.path, &a.1.path))
        }
        SortKey::SizeAsc => {
            a.1.disk_bytes()
                .cmp(&b.1.disk_bytes())
                .then(compare_index_paths(&a.1.path, &b.1.path))
        }
        SortKey::MtimeDesc => sort_mtime(b.1)
            .cmp(&sort_mtime(a.1))
            .then(compare_index_paths(&b.1.path, &a.1.path)),
        SortKey::MtimeAsc => sort_mtime(a.1)
            .cmp(&sort_mtime(b.1))
            .then(compare_index_paths(&a.1.path, &b.1.path)),
    }
}
