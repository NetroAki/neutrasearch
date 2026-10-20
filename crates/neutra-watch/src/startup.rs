use std::path::Path;
use std::process::Command;

pub(super) fn ensure_index(index: &Path) {
    if std::env::var_os("NEUTRASEARCH_MANAGED_WATCH").is_none() {
        return;
    }
    if !index.exists() {
        let sibling = std::env::current_exe()
            .ok()
            .map(|path| path.with_file_name("neutrasearch"));
        let program = sibling
            .filter(|path| path.is_file())
            .unwrap_or_else(|| "/usr/bin/neutrasearch".into());
        let status = Command::new("/usr/bin/timeout")
            .arg("7200")
            .arg(program)
            .arg("--ensure-index")
            .status();
        if !status.is_ok_and(|status| status.success()) {
            eprintln!("neutrasearch: initial metadata indexing failed; repair indexing access in the application");
            std::process::exit(78);
        }
    }
    if let Err(error) = neutra_core::CompactIndex::open_fast(index) {
        eprintln!(
            "neutrasearch: the saved index is unavailable: {error}; rebuild it in the application"
        );
        std::process::exit(78);
    }
}
