use super::*;
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

#[test]
fn native_changes_wake_the_gui_but_unrelated_files_do_not() {
    let root =
        std::env::temp_dir().join(format!("neutra-index-notifications-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let index = root.join("probe.nsx");
    let (tx, rx) = mpsc::channel();
    let guard = IndexEvents::start(&index, EventSender::new(tx, egui::Context::default())).unwrap();
    std::fs::write(index.with_extension("delta"), b"changed").unwrap();
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(3)).unwrap(),
        Event::IndexChanged
    ));
    let deadline = Instant::now() + Duration::from_secs(3);
    while rx.recv_timeout(Duration::from_millis(500)).is_ok() {
        assert!(
            Instant::now() < deadline,
            "index notifications did not settle"
        );
    }
    std::fs::write(root.join("unrelated"), b"not an index").unwrap();
    assert!(rx.recv_timeout(Duration::from_millis(500)).is_err());
    drop(guard);
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        Event::IndexChanged
    ));
    std::fs::remove_dir_all(root).unwrap();
}
