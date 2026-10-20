use neutra_core::{
    BrowserIndex, CompactIndex, DeltaIndex, FileKind, FileRecord, FsKind, Query, SearchHit,
    SearchStats, SortKey, SpillAccumulator,
};
use std::path::PathBuf;

pub(super) struct Fixture {
    pub(super) root: PathBuf,
    pub(super) path: PathBuf,
    pub(super) base: CompactIndex,
    pub(super) browser: BrowserIndex,
}

impl Fixture {
    pub(super) fn build(name: &str, records: &[FileRecord]) -> Self {
        let root = std::env::temp_dir().join(format!(
            "neutrasearch-prepared-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("base.nsx");
        let mut spill = SpillAccumulator::begin(&path).unwrap();
        spill.push_batch(records.to_vec()).unwrap();
        CompactIndex::rebuild_streamed(spill.finish().unwrap(), &path).unwrap();
        let base = CompactIndex::open_fast(&path).unwrap();
        BrowserIndex::ensure(&path, base.generation()).unwrap();
        let browser = BrowserIndex::open(&path, base.generation()).unwrap();
        Self {
            root,
            path,
            base,
            browser,
        }
    }

    pub(super) fn prepared(
        &self,
        query: &Query,
        delta: Option<&DeltaIndex>,
        after: Option<&FileRecord>,
    ) -> (Vec<SearchHit>, SearchStats) {
        self.browser
            .search_prepared(&self.path, &self.base, query, delta, after, &|| false)
            .unwrap()
    }

    pub(super) fn finish(self) {
        drop(self.browser);
        drop(self.base);
        let _ = std::fs::remove_dir_all(self.root);
    }
}

pub(super) fn record(path: &str, size: u64, mtime: i64, kind: FileKind) -> FileRecord {
    FileRecord {
        path: path.into(),
        size,
        disk: size,
        mtime,
        mode: if path.ends_with(".sh") { 0o755 } else { 0 },
        kind,
        fs: FsKind::Btrfs,
        native_id: size,
        native_parent: 0,
        source: 0,
    }
}

pub(super) fn paths(hits: &[SearchHit]) -> Vec<String> {
    hits.iter().map(|hit| hit.record.path.to_string()).collect()
}

pub(super) const ALL_SORTS: [SortKey; 9] = [
    SortKey::Relevance,
    SortKey::NameAsc,
    SortKey::NameDesc,
    SortKey::PathAsc,
    SortKey::PathDesc,
    SortKey::SizeAsc,
    SortKey::SizeDesc,
    SortKey::MtimeAsc,
    SortKey::MtimeDesc,
];
