use anyhow::{bail, Context as _, Result};
use std::{io, process::Output};

pub(super) fn paths(output: io::Result<Output>) -> Result<Vec<String>> {
    let output = output.context("cannot query the Spotlight index")?;
    if !output.status.success() {
        bail!(
            "Spotlight query failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let paths = super::parse_mdfind_nul(&output.stdout);
    if paths.is_empty() {
        bail!("Spotlight supplied no indexed objects for this volume");
    }
    Ok(paths)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    fn output(status: i32, stdout: &[u8], stderr: &[u8]) -> io::Result<Output> {
        Ok(Output {
            status: std::process::ExitStatus::from_raw(status),
            stdout: stdout.to_vec(),
            stderr: stderr.to_vec(),
        })
    }

    #[test]
    fn rejects_unavailable_empty_and_failed_spotlight_queries() {
        assert!(paths(Err(io::Error::from(io::ErrorKind::NotFound))).is_err());
        assert!(paths(output(0, b"", b"")).is_err());
        let error = paths(output(256, b"/partial\0", b"index unavailable"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("index unavailable"));
    }

    #[test]
    fn accepts_successful_indexed_paths_with_embedded_newlines() {
        assert_eq!(
            paths(output(0, b"/A/a file\0/A/with\nnewline\0", b"")).unwrap(),
            vec!["/A/a file", "/A/with\nnewline"]
        );
    }
}
