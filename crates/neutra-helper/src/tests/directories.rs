use super::*;

#[test]
fn directory_summary_serves_live_totals_from_the_durable_pair() {
    use std::sync::Mutex;

    #[derive(Clone)]
    struct SharedOutput(Arc<Mutex<Vec<u8>>>);
    impl Write for SharedOutput {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let base =
        std::env::temp_dir().join(format!("neutra-helper-dirsum-{}.nsx", std::process::id()));
    let _ = std::fs::remove_file(&base);
    let mut delta_path = base.clone();
    delta_path.set_extension("delta");
    let docs = if cfg!(windows) { "C:/docs" } else { "/docs" };
    let docs_dir = FileRecord {
        path: docs.into(),
        size: 0,
        mtime: 0,
        mode: 0,
        kind: FileKind::Dir,
        fs: FsKind::Btrfs,
        native_id: 0,
        native_parent: 0,
        source: 0,
        disk: 0,
    };
    let records = vec![docs_dir, record(&format!("{docs}/a.txt"), 10)];
    build_test_base(&records, &base).unwrap();
    let mut input = Vec::new();
    write_frame(
        &mut input,
        &ClientMsg::Hello {
            proto: PROTO_VERSION,
        },
    )
    .unwrap();
    write_frame(
        &mut input,
        &ClientMsg::ApplyDelta {
            changes: vec![DeltaChange::Upsert(record(&format!("{docs}/b.txt"), 5))],
        },
    )
    .unwrap();
    write_frame(
        &mut input,
        &ClientMsg::DirectorySummary {
            source: 0,
            path: docs.into(),
        },
    )
    .unwrap();
    write_frame(&mut input, &ClientMsg::Shutdown).unwrap();

    let bytes = Arc::new(Mutex::new(Vec::new()));
    run_protocol(
        &mut Cursor::new(input),
        Box::new(SharedOutput(Arc::clone(&bytes))),
        Some(base.clone()),
        None,
        None,
    )
    .unwrap();

    let mut output = Cursor::new(bytes.lock().unwrap().clone());
    let mut entry = None;
    while let Ok(Some(msg)) = read_frame::<_, HelperMsg>(&mut output) {
        if let HelperMsg::Error(error) = &msg {
            panic!("directory-summary protocol failed: {error}");
        }
        if let HelperMsg::DirectorySummary { entry: found } = msg {
            entry = found;
        }
    }
    let entry = entry.expect("directory summary response");
    assert_eq!(entry.logical_bytes, 15);
    assert_eq!(entry.file_count, 2);
    store_cleanup(&base, &delta_path);
}

fn store_cleanup(base: &std::path::Path, delta: &std::path::Path) {
    let mut lock = delta.as_os_str().to_os_string();
    lock.push(".lock");
    for path in [
        base.to_path_buf(),
        delta.to_path_buf(),
        std::path::PathBuf::from(lock),
    ] {
        let _ = std::fs::remove_file(path);
    }
}
