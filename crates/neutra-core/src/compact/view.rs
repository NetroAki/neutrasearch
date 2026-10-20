/// One file row in a directory listing. Sizes track both views: on-disk
/// for the tree, apparent for search-compatible reporting.
#[derive(Debug, Clone)]
pub struct DirFile {
    pub name: Box<str>,
    pub size: u64,
    pub logical: u64,
    pub kind: crate::FileKind,
}
/// One direct subdirectory with its subtree totals.
#[derive(Debug, Clone)]
pub struct DirSubdir {
    pub name: Box<str>,
    pub size: u64,
    pub logical: u64,
    pub files: u64,
}
/// A single directory fetched on demand: file rows plus subdirectory
/// totals, bounded to keep tree browsing under a fixed memory budget.
#[derive(Debug, Clone, Default)]
pub struct DirListing {
    pub files: Vec<DirFile>,
    pub subdirs: Vec<DirSubdir>,
    pub total_size: u64,
    pub total_logical: u64,
    pub total_count: u64,
    pub files_truncated: bool,
}
