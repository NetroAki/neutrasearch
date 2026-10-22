use super::test_support::rec;
use super::*;
use crate::compact_build::temp_path;
#[test]
fn compact_roundtrip_and_substring_search() {
    let records = vec![
        rec("/home/a/AlphaDocument.txt", 1),
        rec("/home/a/beta.rs", 2),
        rec("/opt/gamma/notes.txt", 3),
    ];
    let path = std::env::temp_dir().join(format!(
        "neutra-compact-{}-roundtrip.idx",
        std::process::id()
    ));
    let first = CompactIndex::build(&records, &path).unwrap();
    let stats = CompactIndex::build(&records, &path).unwrap();
    assert_eq!(stats.records, 3);
    assert_ne!(stats.generation, 0);
    assert_ne!(stats.generation, first.generation);
    let index = CompactIndex::open(&path).unwrap();
    assert_eq!(index.generation(), stats.generation);
    let (hits, s) = index.search(&Query::parse("document ext:txt")).unwrap();
    assert_eq!(s.matched, 1);
    assert_eq!(hits[0].record.path.as_ref(), "/home/a/AlphaDocument.txt");
    assert_eq!(hits[0].record.native_id, 101);
    assert_eq!(hits[0].record.native_parent, 11);
    let (hits, _) = index.search(&Query::parse("gamma")).unwrap();
    assert_eq!(hits[0].record.path.as_ref(), "/opt/gamma/notes.txt");
    drop(index);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn stale_marker_blocks_readers_until_full_rebuild() {
    let path =
        std::env::temp_dir().join(format!("neutra-compact-stale-{}.idx", std::process::id()));
    let stale = suffix_path(&path, ".stale");
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&stale);
    let old = CompactIndex::build(&[rec("/old.txt", 1)], &path).unwrap();
    assert_eq!(
        CompactIndex::generation_on_disk(&path).unwrap(),
        old.generation
    );
    std::fs::write(&stale, b"watch overflow").unwrap();
    assert!(CompactIndex::open(&path).is_err());
    assert!(CompactIndex::generation_on_disk(&path).is_err());
    let recovery = CompactIndex::open_for_recovery(&path).unwrap();
    assert_eq!(recovery.generation(), old.generation);
    assert!(stale.is_file());
    drop(recovery);

    let replacement = CompactIndex::build(&[rec("/new.txt", 2)], &path).unwrap();
    assert!(!stale.exists());
    assert_eq!(
        CompactIndex::generation_on_disk(&path).unwrap(),
        replacement.generation
    );
    assert!(CompactIndex::open(&path).is_ok());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn checksum_rejects_corrupted_compact_data() {
    let path =
        std::env::temp_dir().join(format!("neutra-compact-corrupt-{}.idx", std::process::id()));
    let _ = std::fs::remove_file(&path);
    CompactIndex::build(&[rec("/safe.txt", 1)], &path).unwrap();
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[HEADER as usize] ^= 0x40;
    std::fs::write(&path, bytes).unwrap();
    assert!(CompactIndex::open(&path).is_err());
    assert!(CompactIndex::open_for_recovery(&path).is_err());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn build_rejects_non_normalized_paths() {
    let path = std::env::temp_dir().join(format!("neutra-invalid-path-{}.idx", std::process::id()));
    let _ = std::fs::remove_file(&path);
    assert!(CompactIndex::build(&[rec("/allowed/../secret", 1)], &path).is_err());
    assert!(!path.exists());
}

#[cfg(unix)]
#[test]
fn build_refuses_preplanted_temporary_symlink() {
    use std::os::unix::fs::symlink;

    let stem = format!("neutra-symlink-{}", std::process::id());
    let base_path = std::env::temp_dir().join(format!("{stem}.idx"));
    let temporary = temp_path(&base_path);
    let victim = std::env::temp_dir().join(format!("{stem}.victim"));
    let _ = std::fs::remove_file(&base_path);
    let _ = std::fs::remove_file(&temporary);
    let _ = std::fs::remove_file(&victim);
    std::fs::write(&victim, b"do not truncate").unwrap();
    symlink(&victim, &temporary).unwrap();

    assert!(CompactIndex::build(&[rec("/safe.txt", 1)], &base_path).is_err());
    assert_eq!(std::fs::read(&victim).unwrap(), b"do not truncate");
    std::fs::remove_file(temporary).unwrap();
    std::fs::remove_file(victim).unwrap();
}

#[test]
fn publishing_replacement_keeps_existing_mmap_readable() {
    let stem = format!("neutra-publish-{}", std::process::id());
    let base_path = std::env::temp_dir().join(format!("{stem}.idx"));
    let staged_path = std::env::temp_dir().join(format!("{stem}.staged"));
    let _ = std::fs::remove_file(&base_path);
    let _ = std::fs::remove_file(&staged_path);
    CompactIndex::build(&[rec("/old.txt", 1)], &base_path).unwrap();
    let old = CompactIndex::open(&base_path).unwrap();
    CompactIndex::build(&[rec("/new.txt", 2)], &staged_path).unwrap();
    let staged_reader = CompactIndex::open(&staged_path).unwrap();

    let abandoned = temp_path(&base_path);
    std::fs::write(&abandoned, b"incomplete earlier copy").unwrap();

    CompactIndex::publish(&staged_path, &base_path).unwrap();
    assert_eq!(
        std::fs::read(&abandoned).unwrap(),
        b"incomplete earlier copy"
    );
    let new = CompactIndex::open(&base_path).unwrap();
    assert_eq!(
        old.search(&Query::parse("old")).unwrap().0[0]
            .record
            .path
            .as_ref(),
        "/old.txt"
    );
    assert_eq!(
        new.search(&Query::parse("new")).unwrap().0[0]
            .record
            .path
            .as_ref(),
        "/new.txt"
    );
    assert_eq!(
        staged_reader.search(&Query::parse("new")).unwrap().0[0]
            .record
            .path
            .as_ref(),
        "/new.txt"
    );
    drop(new);
    drop(old);
    drop(staged_reader);
    std::fs::remove_file(abandoned).unwrap();
    std::fs::remove_file(base_path).unwrap();
    std::fs::remove_file(staged_path).unwrap();
}

#[test]
fn snapshot_reader_selects_coherent_side_of_compaction_marker() {
    let stem = format!("neutra-pair-{}", std::process::id());
    let base_path = std::env::temp_dir().join(format!("{stem}.idx"));
    let mut delta_path = base_path.clone();
    delta_path.set_extension("delta");
    let staged_path = suffix_path(&base_path, ".compact");
    let marker_path = suffix_path(&base_path, ".compacting");
    for path in [&base_path, &delta_path, &staged_path, &marker_path] {
        let _ = std::fs::remove_file(path);
    }
    CompactIndex::build(&[rec("/old.txt", 1)], &base_path).unwrap();
    let old_generation = CompactIndex::open(&base_path).unwrap().generation();
    let mut delta = DeltaIndex::open(&delta_path, old_generation).unwrap();
    let staged = CompactIndex::build(&[rec("/new.txt", 2)], &staged_path).unwrap();
    std::fs::write(&marker_path, staged.generation.to_le_bytes()).unwrap();

    let (current, current_delta) = CompactIndex::open_with_delta_snapshot(&base_path).unwrap();
    assert_eq!(current.generation(), old_generation);
    assert_eq!(current_delta.unwrap().generation(), old_generation);
    drop(current);

    delta.reset(staged.generation).unwrap();
    let (replacement, replacement_delta) =
        CompactIndex::open_with_delta_snapshot(&base_path).unwrap();
    assert_eq!(replacement.generation(), staged.generation);
    assert_eq!(replacement_delta.unwrap().generation(), staged.generation);
    drop(replacement);
    drop(delta);

    let mut lock_path = delta_path.as_os_str().to_os_string();
    lock_path.push(".lock");
    for path in [
        base_path,
        delta_path,
        staged_path,
        marker_path,
        lock_path.into(),
    ] {
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn rebuild_resets_the_delta_and_rejects_an_active_writer() {
    let stem = format!("neutra-rebuild-{}", std::process::id());
    let base_path = std::env::temp_dir().join(format!("{stem}.idx"));
    let mut delta_path = base_path.clone();
    delta_path.set_extension("delta");
    let lock_path = suffix_path(&delta_path, ".lock");
    for path in [&base_path, &delta_path, &lock_path] {
        let _ = std::fs::remove_file(path);
    }
    CompactIndex::build(&[rec("/old.txt", 1)], &base_path).unwrap();
    let generation = CompactIndex::open(&base_path).unwrap().generation();
    let writer = DeltaIndex::open(&delta_path, generation).unwrap();
    let error = CompactIndex::rebuild(&[rec("/new.txt", 2)], &base_path).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
    drop(writer);

    CompactIndex::rebuild(&[rec("/new.txt", 2)], &base_path).unwrap();
    assert!(!delta_path.exists());
    let (rebuilt, delta) = CompactIndex::open_with_delta_snapshot(&base_path).unwrap();
    assert!(delta.is_none());
    assert_eq!(rebuilt.records().unwrap()[0].path.as_ref(), "/new.txt");

    std::fs::remove_file(base_path).unwrap();
    std::fs::remove_file(lock_path).unwrap();
}

#[test]
fn delta_upserts_and_tombstones_shadow_the_base() {
    let stem = format!("neutra-overlay-{}", std::process::id());
    let base_path = std::env::temp_dir().join(format!("{stem}.idx"));
    let delta_path = std::env::temp_dir().join(format!("{stem}.delta"));
    let _ = std::fs::remove_file(&base_path);
    let _ = std::fs::remove_file(&delta_path);
    CompactIndex::build(&[rec("/a/alpha.txt", 1), rec("/b/beta.txt", 2)], &base_path).unwrap();
    let base = CompactIndex::open(&base_path).unwrap();
    let mut delta = DeltaIndex::open(&delta_path, base.generation()).unwrap();
    delta
        .apply(crate::DeltaChange::Remove("/a/alpha.txt".into()))
        .unwrap();
    delta
        .apply(crate::DeltaChange::Upsert(rec("/b/beta.txt", 20)))
        .unwrap();
    delta
        .apply(crate::DeltaChange::Upsert(rec("/c/gamma.txt", 3)))
        .unwrap();

    let (hits, stats) = base
        .search_with_delta(&Query::parse("ext:txt"), &delta)
        .unwrap();
    assert_eq!(stats.matched, 2);
    assert_eq!(hits.len(), 2);
    assert!(!hits
        .iter()
        .any(|hit| hit.record.path.as_ref() == "/a/alpha.txt"));
    assert!(hits
        .iter()
        .any(|hit| hit.record.path.as_ref() == "/b/beta.txt" && hit.record.size == 20));
    assert!(hits
        .iter()
        .any(|hit| hit.record.path.as_ref() == "/c/gamma.txt"));

    drop(delta);
    drop(base);
    std::fs::remove_file(base_path).unwrap();
    std::fs::remove_file(delta_path).unwrap();
}
