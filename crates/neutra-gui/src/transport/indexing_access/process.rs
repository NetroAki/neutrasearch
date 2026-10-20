use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub(super) fn run(mut command: Command, seconds: u64) -> Result<(), String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("Cannot contact the system access manager: {error}"))?;
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                return Err(format!(
                    "The system access manager declined the request ({status})"
                ))
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.to_string());
            }
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("The system access request timed out".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_exit_status_and_reaps_timeout() {
        assert!(run(Command::new("/usr/bin/true"), 1).is_ok());
        assert!(run(Command::new("/usr/bin/false"), 1).is_err());
        let mut sleep = Command::new("/usr/bin/sleep");
        sleep.arg("10");
        assert!(run(sleep, 0).unwrap_err().contains("timed out"));
    }
}
