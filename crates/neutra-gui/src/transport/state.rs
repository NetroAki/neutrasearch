use std::sync::{Arc, Mutex};

pub(crate) fn helper_start_failure(summary: &str, stderr: &Arc<Mutex<String>>) -> String {
    let detail = stderr
        .lock()
        .map(|text| text.trim().to_owned())
        .unwrap_or_default();
    if detail.contains("textual authentication agent")
        || detail.contains("current controlling terminal")
    {
        return "administrator approval could not open in this desktop session; launch Neutrasearch from the desktop and try again".into();
    }
    if detail.is_empty() {
        summary.to_owned()
    } else {
        format!("{summary}: {detail}")
    }
}
