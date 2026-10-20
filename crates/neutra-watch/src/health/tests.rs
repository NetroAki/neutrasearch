use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn monitor_child() {
    let Some(path) = std::env::var_os("NEUTRA_WATCH_HEALTH_TEST") else {
        return;
    };
    let path = std::path::PathBuf::from(path);
    super::monitor(&path);
    std::fs::write(path.with_extension("ready"), b"ready").unwrap();
    std::thread::sleep(Duration::from_secs(10));
    panic!("health monitor did not terminate an unavailable service");
}

#[test]
fn late_stale_marker_stops_the_supervisor_with_configuration_failure() {
    let path = std::env::temp_dir().join(format!("ns-health-{}.nsx", std::process::id()));
    let ready = path.with_extension("ready");
    let marker = std::path::PathBuf::from(format!("{}.stale", path.display()));
    let _ = std::fs::remove_file(&ready);
    let _ = std::fs::remove_file(&marker);
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "health::tests::monitor_child", "--nocapture"])
        .env("NEUTRA_WATCH_HEALTH_TEST", &path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    std::fs::write(&marker, b"watch queue overflow").unwrap();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if Instant::now() >= deadline {
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    if status.is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }
    let _ = std::fs::remove_file(&ready);
    let _ = std::fs::remove_file(&marker);
    assert_eq!(status.and_then(|status| status.code()), Some(78));
}
