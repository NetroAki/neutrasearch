use super::*;

#[test]
fn one_shot_protocol_ends_after_scan_preparation_error() {
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
        &ClientMsg::Scan {
            mounts: vec![MountInfo {
                device: "untrusted".into(),
                mountpoint: "/not-a-trusted-mount".into(),
                fs: FsKind::Ext4,
                source: neutra_core::MountSource::Local,
            }],
            roots: vec!["/not-a-trusted-mount".into()],
            allow_zfs_enumerate: false,
        },
    )
    .unwrap();

    let bytes = Arc::new(Mutex::new(Vec::new()));
    run_protocol(
        &mut Cursor::new(input),
        Box::new(SharedOutput(Arc::clone(&bytes))),
        None,
        None,
        None,
    )
    .unwrap();

    let mut output = Cursor::new(bytes.lock().unwrap().clone());
    assert!(matches!(
        read_frame::<_, HelperMsg>(&mut output).unwrap(),
        Some(HelperMsg::Hello { .. })
    ));
    assert!(matches!(
        read_frame::<_, HelperMsg>(&mut output).unwrap(),
        Some(HelperMsg::Error(error)) if error.contains("not present in the trusted OS mount table")
    ));
}
