use super::{programs, staging};
use anyhow::{bail, Context, Result};
use neutra_core::{FileRecord, MountInfo, ScanStats};
use std::io::{BufReader, Read};
use std::process::{Child, Stdio};
use std::time::Instant;

struct Reader(Child);

impl Drop for Reader {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub(super) fn scan(mount: &MountInfo, sink: &mut dyn FnMut(FileRecord)) -> Result<ScanStats> {
    super::dataset::validate(&mount.device)?;
    let reader = programs::reader()?;
    let started = Instant::now();
    let mut reader = Reader(
        programs::command(&reader)
            .arg(&mount.device)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Start native ZFS metadata reader")?,
    );
    let stdout = reader
        .0
        .stdout
        .take()
        .context("Missing ZFS metadata stream")?;
    let mut spool = staging::anonymous_spool()?;
    let mut stderr = reader.0.stderr.take().context("Missing ZFS error stream")?;
    let errors = std::thread::spawn(move || {
        let mut retained = Vec::new();
        let mut buffer = [0; 4096];
        while let Ok(count) = stderr.read(&mut buffer) {
            if count == 0 {
                break;
            }
            let keep = count.min(16384usize.saturating_sub(retained.len()));
            retained.extend_from_slice(&buffer[..keep]);
        }
        retained
    });
    let capture = staging::capture(&mut BufReader::new(stdout), &mut spool);
    if capture.is_err() {
        let _ = reader.0.kill();
    }
    let status = reader.0.wait()?;
    let error = errors.join().unwrap_or_default();
    if !status.success() {
        bail!(
            "Native ZFS metadata indexing failed ({status}): {}",
            String::from_utf8_lossy(&error).trim()
        );
    }
    capture?;
    let mut stats = staging::validate_and_emit(&mut spool, &mount.mountpoint, true, sink)?;
    stats.wall_ms = started.elapsed().as_millis() as u64;
    stats.detail = "OpenZFS ZAP/SA metadata snapshot; no file-tree enumeration".into();
    Ok(stats)
}
