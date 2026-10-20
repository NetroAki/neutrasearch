use std::path::Path;

#[cfg(test)]
mod tests;

pub(crate) fn monitor(index: &Path) {
    let mut marker = index.as_os_str().to_os_string();
    marker.push(".stale");
    let marker = std::path::PathBuf::from(marker);
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
        match std::fs::metadata(&marker) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            state => {
                eprintln!(
                    "neutrasearch-watch: native indexing is unavailable ({}): {state:?}",
                    marker.display()
                );
                // Closing the supervisor's pipes also stops its helper and event readers.
                std::process::exit(78);
            }
        }
    });
}
