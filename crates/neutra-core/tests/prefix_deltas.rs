use neutra_core::{
    BrowserIndex, CompactIndex, DeltaChange, DeltaIndex, DirectorySummary, DirectorySummaryOverlay,
    FileKind, FileRecord, FsKind, Query, SortKey, SpillAccumulator,
};
use std::path::PathBuf;

struct Fixture {
    root: PathBuf,
    path: PathBuf,
}
impl Fixture {
    fn new(label: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("neutra-prefix-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).unwrap();
        let path = root.join("base.nsx");
        Self { root, path }
    }
    fn build(&self) -> CompactIndex {
        let mut spill = SpillAccumulator::begin(&self.path).unwrap();
        spill
            .push_batch(vec![
                record("/d", 0, FileKind::Dir),
                record("/d/a.dat", 10, FileKind::File),
                record("/d/sub", 0, FileKind::Dir),
                record("/d/sub/b.dat", 20, FileKind::File),
                record("/d/é.dat", 7, FileKind::File),
                record("/d-b.dat", 30, FileKind::File),
            ])
            .unwrap();
        CompactIndex::rebuild_streamed(spill.finish().unwrap(), &self.path).unwrap();
        CompactIndex::open_fast(&self.path).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
fn record(path: &str, size: u64, kind: FileKind) -> FileRecord {
    FileRecord {
        path: path.into(),
        size,
        disk: size,
        mtime: size as i64,
        mode: 0,
        kind,
        fs: FsKind::Btrfs,
        native_id: size + 100,
        native_parent: 100,
        source: 0,
    }
}

#[cfg(unix)]
#[test]
fn prefix_ordering_and_recreation_survive_snapshot_checkpoint_and_restart() {
    let fixture = Fixture::new("ordering");
    let path = fixture.path.with_extension("delta");
    let mut delta = DeltaIndex::open(&path, 11).unwrap();
    delta
        .apply_batch([
            DeltaChange::Upsert(record("/d/sub/old.dat", 2, FileKind::File)),
            DeltaChange::RemovePrefix("/d".into()),
            DeltaChange::Upsert(record("/d/sub/new.dat", 3, FileKind::File)),
            DeltaChange::RemovePrefix("/d/sub".into()),
            DeltaChange::Upsert(record("/d/sub/final.dat", 4, FileKind::File)),
        ])
        .unwrap();
    delta.sync().unwrap();
    verify(&delta);
    let snapshot = DeltaIndex::open_snapshot(&path, 11).unwrap();
    verify(&snapshot);
    drop(snapshot);
    delta.checkpoint().unwrap();
    verify(&delta);
    drop(delta);
    let reopened = DeltaIndex::open(&path, 11).unwrap();
    verify(&reopened);
    assert!(std::fs::metadata(path).unwrap().len() < 1024);
}
#[cfg(unix)]
fn verify(delta: &DeltaIndex) {
    assert!(delta.is_removed("/d/sub/old.dat").unwrap());
    assert!(delta.is_removed("/d/sub/new.dat").unwrap());
    assert!(!delta.is_removed("/d/sub/final.dat").unwrap());
    assert_eq!(
        delta.upsert_for("/d/sub/final.dat").unwrap().unwrap().size,
        4
    );
    assert!(!delta.shadows("/d-b.dat").unwrap());
    assert_eq!(
        delta
            .shadows_batch(&["/d/sub/old.dat", "/d/sub/final.dat", "/d-b.dat", "/d/é.dat"])
            .unwrap(),
        vec![true, true, false, true]
    );
}

#[test]
fn search_listing_catalog_and_directory_totals_agree_after_prefix_removal() {
    let fixture = Fixture::new("consumers");
    let base = fixture.build();
    BrowserIndex::ensure(&fixture.path, base.generation()).unwrap();
    let mut delta =
        DeltaIndex::open(&fixture.path.with_extension("delta"), base.generation()).unwrap();
    let changes = vec![
        DeltaChange::RemovePrefix("/d".into()),
        DeltaChange::Upsert(record("/d", 0, FileKind::Dir)),
        DeltaChange::Upsert(record("/d/new.dat", 5, FileKind::File)),
    ];
    delta.apply_batch(changes.clone()).unwrap();
    delta.sync().unwrap();
    BrowserIndex::apply_changes(&fixture.path, base.generation(), &changes).unwrap();
    BrowserIndex::mark_delta(&fixture.path, &delta).unwrap();
    let browser = BrowserIndex::open(&fixture.path, base.generation()).unwrap();
    assert!(browser.record_by_path("/d/a.dat").unwrap().is_none());
    assert!(browser.record_by_path("/d/sub/b.dat").unwrap().is_none());
    assert!(browser.record_by_path("/d-b.dat").unwrap().is_some());
    for sort in [
        SortKey::Relevance,
        SortKey::NameAsc,
        SortKey::NameDesc,
        SortKey::PathAsc,
        SortKey::PathDesc,
        SortKey::SizeAsc,
        SortKey::SizeDesc,
        SortKey::MtimeAsc,
        SortKey::MtimeDesc,
    ] {
        let q = Query {
            sort,
            limit: 100,
            ..Query::default()
        };
        let expected = base.search_with_delta(&q, &delta).unwrap().0;
        let actual = browser.search(&base, &q, Some(&delta)).unwrap().0;
        assert_eq!(paths(&actual), paths(&expected));
    }
    let listing = base.list_directory("/d", None, Some(&delta)).unwrap();
    assert_eq!(listing.total_count, 1);
    assert_eq!(listing.total_logical, 5);
    let summary = DirectorySummary::open_for_compact(&fixture.path, base.generation()).unwrap();
    let overlay = DirectorySummaryOverlay::from_delta(summary, &base, &delta).unwrap();
    assert_eq!(overlay.get(0, "/d").unwrap().unwrap().logical_bytes, 5);
    assert!(overlay.get(0, "/d/sub").unwrap().is_none());
    assert_eq!(overlay.get(0, "/").unwrap().unwrap().logical_bytes, 35);
    BrowserIndex::apply_delta(&fixture.path, &delta).unwrap();
    assert!(browser.record_by_path("/d/new.dat").unwrap().is_some());
    let merged = fixture.root.join("merged.nsx");
    let mut spill = SpillAccumulator::begin(&merged).unwrap();
    CompactIndex::spill_compacted_base(&base, &delta, &mut spill).unwrap();
    CompactIndex::rebuild_streamed(spill.finish().unwrap(), &merged).unwrap();
    let published = CompactIndex::open_fast(&merged).unwrap();
    assert_eq!(published.len(), 3);
    assert!(published
        .record_by_path_any_source("/d/a.dat")
        .unwrap()
        .is_none());
}
fn paths(hits: &[neutra_core::SearchHit]) -> Vec<&str> {
    hits.iter().map(|hit| &*hit.record.path).collect()
}

#[test]
fn version_three_wal_is_readable_and_migrates_before_prefix_appends() {
    let fixture = Fixture::new("legacy");
    let path = &fixture.path;
    let payload =
        bincode::serialize(&DeltaChange::Upsert(record("/d/old", 1, FileKind::File))).unwrap();
    let mut bytes = b"NEUTDLT3".to_vec();
    bytes.extend(42u64.to_le_bytes());
    bytes.extend((payload.len() as u32).to_le_bytes());
    bytes.extend(crc32fast::hash(&payload).to_le_bytes());
    bytes.extend(payload);
    std::fs::write(path, bytes).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let snapshot = DeltaIndex::open_snapshot(path, 42).unwrap();
    assert!(snapshot.upsert_for("/d/old").unwrap().is_some());
    drop(snapshot);
    assert_eq!(&std::fs::read(path).unwrap()[..8], b"NEUTDLT3");
    let mut writer = DeltaIndex::open(path, 42).unwrap();
    assert_eq!(&std::fs::read(path).unwrap()[..8], b"NEUTDLT4");
    writer
        .apply(DeltaChange::RemovePrefix("/d".into()))
        .unwrap();
    writer.sync().unwrap();
    drop(writer);
    let reopened = DeltaIndex::open(path, 42).unwrap();
    assert!(reopened.is_removed("/d/old").unwrap());
}

#[test]
fn invalid_prefix_does_not_change_the_log() {
    let fixture = Fixture::new("invalid");
    let mut delta = DeltaIndex::open(&fixture.path, 1).unwrap();
    assert!(delta
        .apply(DeltaChange::RemovePrefix("/d/../elsewhere".into()))
        .is_err());
    assert_eq!(delta.wal_bytes(), 16);
    assert_eq!(delta.change_count().unwrap(), 0);
}

#[test]
fn failed_overlay_write_cannot_advance_a_later_cursor() {
    let fixture = Fixture::new("failed-overlay");
    let mut delta = DeltaIndex::open(&fixture.path, 10).unwrap();
    let overlay = fixture.root.join(".delta.overlay/base.nsx.resolved.sqlite");
    let db = rusqlite::Connection::open(overlay).unwrap();
    db.execute_batch("CREATE TRIGGER reject_change BEFORE INSERT ON changes BEGIN SELECT RAISE(ABORT,'injected write failure'); END;").unwrap();
    assert!(delta
        .apply(DeltaChange::Upsert(record("/first", 1, FileKind::File)))
        .is_err());
    let failed_bytes = std::fs::metadata(&fixture.path).unwrap().len();
    db.execute_batch("DROP TRIGGER reject_change;").unwrap();
    assert!(delta
        .apply(DeltaChange::Upsert(record("/second", 2, FileKind::File)))
        .is_err());
    assert_eq!(
        std::fs::metadata(&fixture.path).unwrap().len(),
        failed_bytes
    );
    drop(db);
    drop(delta);
    let restored = DeltaIndex::open(&fixture.path, 10).unwrap();
    assert!(restored.upsert_for("/first").unwrap().is_some());
    assert!(restored.upsert_for("/second").unwrap().is_none());
}

#[test]
fn bounded_directory_rows_preserve_totals_and_source_scope() {
    let fixture = Fixture::new("bounded-listing");
    let base = fixture.build();
    let mut delta =
        DeltaIndex::open(&fixture.path.with_extension("delta"), base.generation()).unwrap();
    let mut extra = record("/d/z.dat", 10, FileKind::File);
    extra.source = 1;
    delta.apply(DeltaChange::Upsert(extra)).unwrap();
    let listing = base
        .list_directory_with_limit("/d", None, Some(&delta), 1)
        .unwrap();
    assert!(listing.files_truncated);
    assert_eq!(listing.files.len(), 1);
    assert_eq!(&*listing.files[0].name, "a.dat");
    assert_eq!(listing.total_count, 4);
    assert_eq!(listing.total_size, 47);
    assert_eq!(listing.subdirs[0].size, 20);
    let empty = base
        .list_directory_with_limit("/d", None, Some(&delta), 0)
        .unwrap();
    assert!(empty.files.is_empty());
    assert!(empty.files_truncated);
    assert_eq!(empty.total_size, listing.total_size);
    let scoped = base
        .list_directory_with_limit("/d", Some(1), Some(&delta), 1)
        .unwrap();
    assert_eq!(scoped.total_count, 1);
    assert_eq!(scoped.total_size, 10);
    assert_eq!(&*scoped.files[0].name, "z.dat");
    assert!(!scoped.files_truncated);
}

#[path = "prefix_deltas/catalog.rs"]
mod catalog;
