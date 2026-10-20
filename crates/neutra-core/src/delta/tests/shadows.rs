use super::*;

fn names(prefix: &str, count: usize) -> Vec<String> {
    (0..count).map(|i| format!("{prefix}/file-{i}")).collect()
}

fn answers(delta: &DeltaIndex, paths: &[String]) -> Vec<bool> {
    let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
    delta.shadows_batch(&refs).unwrap()
}

#[test]
fn batched_shadow_answers_match_single_lookups_through_updates_and_prefix_removal() {
    let path =
        std::env::temp_dir().join(format!("neutra-delta-shadows-{}.wal", std::process::id()));
    remove_log(&path);
    let mut delta = DeltaIndex::open(&path, 10).unwrap();
    for (i, name) in names("/a", 40).iter().enumerate() {
        delta
            .apply(DeltaChange::Upsert(record(name, i as u64)))
            .unwrap();
    }
    delta.apply(DeltaChange::Remove("/b/gone".into())).unwrap();
    let mut probe = names("/a", 60);
    probe.extend(names("/c", 20));
    probe.push("/b/gone".into());
    let expect = |delta: &DeltaIndex, probe: &[String]| -> Vec<bool> {
        probe.iter().map(|p| delta.shadows(p).unwrap()).collect()
    };
    let first = answers(&delta, &probe);
    assert_eq!(first, expect(&delta, &probe));
    assert!(first[..40].iter().all(|hit| *hit));
    assert!(first[40..80].iter().all(|hit| !*hit));
    assert!(first[first.len() - 1]);

    // A change applied after the in-memory filter exists must be visible.
    delta
        .apply(DeltaChange::Upsert(record("/c/file-3", 1)))
        .unwrap();
    let second = answers(&delta, &probe);
    assert!(second[60 + 3]);
    assert_eq!(second, expect(&delta, &probe));

    // A prefix removal shadows everything beneath it.
    delta.apply(DeltaChange::RemovePrefix("/c".into())).unwrap();
    let third = answers(&delta, &probe);
    assert!(third[60..80].iter().all(|hit| *hit));
    assert_eq!(third, expect(&delta, &probe));
    drop(delta);
    remove_log(&path);
}

#[test]
fn parallel_upsert_scan_visits_each_live_record_once() {
    let path = std::env::temp_dir().join(format!("neutra-delta-scan-{}.wal", std::process::id()));
    remove_log(&path);
    let mut delta = DeltaIndex::open(&path, 10).unwrap();
    for (i, name) in names("/s", 200).iter().enumerate() {
        delta
            .apply(DeltaChange::Upsert(record(name, i as u64)))
            .unwrap();
    }
    delta
        .apply(DeltaChange::Remove("/s/file-7".into()))
        .unwrap();
    let parts = delta
        .scan_upserts(8, Vec::<String>::new, |seen, view| {
            seen.push(view.path.to_owned());
            Ok(())
        })
        .unwrap();
    let mut all: Vec<String> = parts.into_iter().flatten().collect();
    all.sort();
    let mut expected = Vec::new();
    delta
        .for_each_upsert(|record| {
            expected.push(record.path.to_string());
            Ok(())
        })
        .unwrap();
    expected.sort();
    assert_eq!(all.len(), 199);
    assert_eq!(all, expected);
    drop(delta);
    remove_log(&path);
}
