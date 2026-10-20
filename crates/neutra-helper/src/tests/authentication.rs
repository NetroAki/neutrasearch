use super::*;

#[test]
fn authentication_runs_after_hello_before_commands() {
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
    write_frame(&mut input, &ClientMsg::Shutdown).unwrap();

    let bytes = Arc::new(Mutex::new(Vec::new()));
    let authenticated = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let callback_authenticated = Arc::clone(&authenticated);
    let callback_output = Arc::clone(&bytes);
    let authenticate = move || {
        let mut output = Cursor::new(callback_output.lock().unwrap().clone());
        assert!(matches!(
            read_frame::<_, HelperMsg>(&mut output).unwrap(),
            Some(HelperMsg::Hello { .. })
        ));
        callback_authenticated.store(true, Ordering::Release);
        Ok(())
    };

    run_protocol_with_auth(
        &mut Cursor::new(input),
        Box::new(SharedOutput(Arc::clone(&bytes))),
        None,
        None,
        None,
        Some(&authenticate),
    )
    .unwrap();
    assert!(authenticated.load(Ordering::Acquire));
}

#[test]
fn protocol_work_is_bounded_before_execution() {
    let mut query = Query::parse("needle");
    query.limit = 0;
    assert!(validate_query(&query).is_err());
    query.limit = MAX_QUERY_RESULTS + 1;
    assert!(validate_query(&query).is_err());
    query.limit = 1;
    query.scope_roots = vec!["relative/scope".into()];
    assert!(validate_query(&query).is_err());
    assert!(validate_delta_changes(&[DeltaChange::Remove("relative/path".into())]).is_err());
    assert!(validate_delta_changes(&[DeltaChange::Remove("/allowed/../secret".into())]).is_err());
    assert!(resolve_scan_mounts(Vec::new(), Vec::new(), false).is_err());
}
