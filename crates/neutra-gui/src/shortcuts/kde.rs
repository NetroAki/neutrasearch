use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub(super) fn register(keys: &[String]) -> Result<(), String> {
    let values = keys
        .iter()
        .map(|key| {
            super::keys::parse(key)
                .and_then(super::keys::qt_key)
                .map(|n| n.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(include_str!(
            "../../../../packaging/linux/neutrasearch-register-shortcut"
        ))
        .arg("neutrasearch-register-shortcut")
        .args(if values.is_empty() {
            vec!["--disabled".into()]
        } else {
            values
        })
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|e| format!("Cannot register shortcuts: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            let output = child.wait_with_output().map_err(|e| e.to_string())?;
            return if status.success() {
                Ok(())
            } else {
                Err(String::from_utf8_lossy(&output.stderr).trim().into())
            };
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("KDE shortcut registration timed out".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
